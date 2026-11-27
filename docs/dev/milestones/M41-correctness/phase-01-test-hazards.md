# Phase 01 — test-hazards

**Milestone:** M41 — Correctness & reliability
**Status:** in-progress
**Drafted:** 2026-11-27
**Depends on:** none
**Estimated diff:** ~200 lines (tests only)
**Tags:** language=rust, kind=test-fix, size=s

## Goal

Close audit rows **Q9**, **Q10** and **Q11**. These are environment
hazards in the test suite. Every row was probed on 2026-11-27, and every
scratch test was reverted afterwards.

1. **Q9: 17 read-file tests run with the real `HOME`, unguarded.**
   - `with_home_set` (`src/daemon/executor/file_ops/read_tests.rs`) sets
     `HOME`, restores it and drops the guard before it returns. Its doc
     comment says the opposite.
   - **Probed:** right after `with_home_set(&tmp).await`, `HOME` was
     `/home/matt`, not the temp dir.
   - So each of its 17 callers runs its body with the operator's `HOME`
     and without `TEST_HOME_LOCK`. Meanwhile other tests can set the
     environment.
2. **Q10 (latent): a host task outlives its guard.**
   - `launch_run_host_serves_status_over_its_socket` (`src/shell/launch.rs`)
     runs on a multi-thread runtime.
   - It `tokio::spawn`s `run_host` and never stops it. The test home guard
     and the temp dir drop while that task may still run on a worker
     thread.
   - **Probed:** the test left no stray `bash`, and no failure was seen.
     This row is **latent**: its test is a preservation test.
3. **Q11: cleanup removes `$SHELL`, `$TMUX` and `$TMUX_PANE` instead of
   restoring them.**
   - The affected code:
     - `with_fish_shell` and `a_local_shell_still_uses_the_login_shell`
       in `src/shell/session_tests.rs`;
     - `FishShellEnv`'s `Drop` in
       `src/daemon/executor/shell_exec_tests.rs`;
     - the two `webhook_ghost_*` tests in `tests/isolation.rs`.
   - **Probed:** a scratch test run after the session tests
     (`--test-threads=1`, `SHELL=/bin/bash`) saw `SHELL` = `None`.

## Statement of work

1. **Q9.** Every former `with_home_set` caller runs its whole body inside
   `with_home(&tmp, || async { … }).await`. Then delete `with_home_set`.
   No assertion changes. A body that stops passing once `HOME` really is
   the temp dir is a finding: report it rather than weakening it.
2. **Q10.** Stop the spawned host before the guard and the temp dir drop.
   That means aborting it and awaiting its handle at the end of the test.
   Any equivalent works, as long as no task the test spawned is still
   running when the guard drops. Keep the test's assertions.
3. **Q11.** Each site records the variable's value before it changes it,
   and puts that value back afterwards: set it if it was set, remove it if
   it was not. `a_local_shell_still_uses_the_login_shell` keeps its
   fallback assertion (no `$SHELL` gives `bash`) and restores after it.
   The isolation tests restore `TMUX` and `TMUX_PANE` alongside `HOME`
   and `TMUX_TMPDIR`.

**Closed file list:** `read_tests.rs`, `launch.rs` (its tests module
only), `session_tests.rs`, `shell_exec_tests.rs` and `tests/isolation.rs`.
No production code changes.

**Gotchas:**
- `crate::test_home_guard()` is not re-entrant. A test takes it **once**.
  `with_home` already takes it, so a body inside `with_home` must not take
  it again.
- `set_var` and `remove_var` stay inside the `unsafe` blocks the files
  already use.

**Edit order:** each step ends with `cargo build --tests`.
1. **Your first tool call is an edit adding the red test**
   `with_fish_shell_restores_the_previous_shell` to
   `src/shell/session_tests.rs`.
2. The Q11 fixes and the second red test.
3. Q9, then Q10.

**Read limit:** read at most **3** files before your first edit.

**Budget:** you have **1200 turns**; use them efficiently.

## First red test

`with_fish_shell_restores_the_previous_shell`, in `session_tests.rs`.
- **Setup:** under the home guard, set `SHELL` to `/bin/probe-shell`.
  Call `with_fish_shell` with an empty body.
- **Asserts:** `SHELL` is `/bin/probe-shell` again.
- **End state:** the test leaves `SHELL` as it found it.
- **Today:** red, because `SHELL` is `None`.

Paste the red run.

## Acceptance criteria

- [ ] **Q11, session:** `with_fish_shell_restores_the_previous_shell`
      passes. Paste its red run.
- [ ] **Q11, guard type:** `fish_shell_env_restores_the_previous_shell`
      (in `shell_exec_tests.rs`) passes. It is the same check for
      dropping a `FishShellEnv`, with a second leg: when `SHELL` was unset
      before, it is unset after. Paste its red run.
- [ ] **Q11, isolation:** each `webhook_ghost_*` test ends by asserting
      that `TMUX` and `TMUX_PANE` hold their entry values. Run this once.
      It must pass now, and its red run before the fix must be pasted:
      `TMUX=probe-sentinel TMUX_PANE=%99 cargo test --test isolation webhook_ghost`
- [ ] **Q9:** this prints **0**. Today it prints 18, the definition and
      its 17 callers:
      `grep -c 'with_home_set' src/daemon/executor/file_ops/read_tests.rs`
- [ ] **Q9, stability (preservation; it passes today too):** run this
      once. It prints **0**. The module has 39 tests plus 1 ignored:
      `for i in $(seq 20); do cargo test --lib daemon::executor::file_ops::read::tests 2>&1 | grep -q 'test result: ok. 39 passed' || echo FAIL; done | grep -c FAIL`
- [ ] **Q10, preservation:** run this once. It prints **0**:
      `for i in $(seq 20); do cargo test --lib shell::launch::tests::launch_run_host_serves_status_over_its_socket -- --exact 2>&1 | grep -q 'test result: ok. 1 passed' || echo FAIL; done | grep -c FAIL`
- [ ] **No test removed (preservation):** run this once. It prints
      **0**:
      `git diff 21e4c2e8 -- src tests | grep -cE '^-\s*#\[(tokio::)?test'`
- [ ] **Count:** the lib suite reports **3211 passed**, which is today's
      3209 plus the two named tests. Not more.
- [ ] **Gates:** all four pass, each run once.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is
   written.
2. **Implement:** follow the edit order, writing each red test before its
   fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the three red runs, **as output, not summarized**;
   - the two loop outputs and the greps.

   Then commit as one `test:` commit.

## Authorizations

- You may touch the five files in the closed file list, and no others.
- You may delete `with_home_set`.

## Out of scope

- Production code.
- The synchronous `with_home` helpers in other test files. They run their
  work inside the closure.
- The milestone's 20-run full-suite exit criterion, which is checked at
  the milestone close.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-27 07:58 (started)

Work has begun on the phase: the first red test
(`with_fish_shell_restores_the_previous_shell`) is written and fails red as
expected; the status is now `in-progress`.

Task list (each line names the acceptance item it closes):

1. Q11-session: add `with_fish_shell_restores_the_previous_shell` red test, then
   make `with_fish_shell` in `session_tests.rs` restore the previous `$SHELL`.
2. Q11-guard: add `fish_shell_env_restores_the_previous_shell` red test, then fix
   `FishShellEnv`'s `Drop` in `shell_exec_tests.rs` to restore-or-remove
   (second leg: unset-before stays unset-after).
3. Q11-isolation: make each `webhook_ghost_*` test in `tests/isolation.rs`
   restore `TMUX` and `TMUX_PANE` from their entry values, and add the end-of-test
   assertions; capture the red run.
4. Q11-session-2: make `a_local_shell_still_uses_the_login_shell` restore `$SHELL`
   after its fallback assertion.
5. Q9: convert every `with_home_set` caller in `read_tests.rs` to run inside
   `with_home(&tmp, || async { ... }).await`, then delete `with_home_set`; grep
   prints 0.
6. Q10: in `launch.rs`'s tests module, abort the spawned `run_host` task and await
   its handle before the guard and temp dir drop; run the 20× preservation loop.
7. Preservation: confirm no `#[test]`/`#[tokio::test]` lines removed vs `21e4c2e8`,
   and the lib suite reports 3211 passed.
8. Capture evidence: run the four gates once each; paste into the
   end-to-end verification entry; commit as one `test:` commit.
