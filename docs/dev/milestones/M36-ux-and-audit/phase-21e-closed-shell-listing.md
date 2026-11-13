# Phase 21e — closed-shell-listing

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-12
**Depends on:** phase 21d (the 60 s grace)
**Estimated diff:** ~150 lines, most of it tests
**Tags:** language=rust, kind=fix, size=s

## Goal

When the milestone close was re-checked after 21d (2026-11-12), one exit
criterion still failed:

> After `daemoneye chat` exits, its shells are gone from `list_shells`
> within a minute.

The daemon now closes the chat's shell at 60 s. The log reads `stayed
detached past its grace; closed shells ["s1"]`, and the host process and
socket are gone. But `list_shells` and `top` keep listing `s1` as a live
shell at its prompt. The registry learns that a host died only from
`reap_exited`, on the maintenance tick's **hourly** pass. 17a's test checks
that the host is gone, never what the listing shows, which is why it passed.

The PE's decision (2026-11-12) is **stamp + hide**:
- An observed close marks the record exited at once.
- `list_shells` and `top` omit exited rows.
- The record itself stays, so `read_shell` by id keeps working until the
  reaper removes it.

## Behaviour

1. **Stamp on an observed close.** Sometimes a close sees the shell's host
   gone: the lock probe after the hang-up reads the host dead, which is
   `closed_by_observed_end`'s "Closed shell …" branch. When that happens,
   the record's `exited_unix` is set to now, exactly as `reap_exited` would
   stamp it. The record is **not** removed.
2. **No stamp otherwise.** A close that does not see the host gone (the
   "Asked shell … to close, but its host is still holding its lock" branch)
   leaves `exited_unix` unset. A shell whose host may still be alive is
   never marked exited.
3. **Both close paths.** The `close_shell` tool and the detach grace
   (`close_after_detach`) both go through `close_shell_with`, and so does
   the `wait: false` completion path in `dispatch_background.rs`. One change
   covers all three.
4. **`list_shells` hides exited rows.** The `list_shells` tool omits every
   shell whose `exited_unix` is set, in both scopes. That includes shells
   the reaper stamped after a crash. `format_shell_rows` can still render
   `[exited]` for a row it is given; the filter belongs to the row
   collection, so the existing formatter tests keep passing.
5. **`top` hides exited rows.** `top`'s Shells panel omits exited rows. The
   filter runs where `top` loads its `ShellList` (`top_load.rs`), so the
   rows the panel draws are the rows selection and Enter-attach index into.
6. **Unchanged:**
   - `read_shell`, `read_shell_screen` and `read_shell_log` on an exited
     shell's id work as before.
   - `/shells` (the slash command) still shows exited shells with
     `exited N ago`, for the user's own diagnostics.
   - The prompt's shells context block is not touched.
   - `reap_exited`'s cadence and `exited_retention_secs` are not touched.

## Constraints

- **Lock discipline.** `close_shell_with` holds no registry lock across an
  `.await`. The stamp is one short `with_shells` call after the
  observation, the same shape as the ownership check at the top of the
  function.
- The `close_shell_ends_the_callers_shell` test pins "close_shell must not
  remove the record". That contract stays: stamp, never remove.
- Never run `tmux` without `-S <socket>`. No test here needs tmux.

## Notes for executor

**Symbol map** (all of these exist today):

- `src/daemon/executor/knowledge/shell_life.rs`:
  - `close_shell_with`: the ownership check, `close_or_term`, then
    `closed_by_observed_end`.
  - `closed_by_observed_end`: polls `host_is_alive(lock)` for 1.5 s and
    returns the sentence.
  - `close_after_detach`: the grace.
- `src/shell/registry.rs`: `with_shells(reg, |store| …)`, the only way to
  touch the store, which is a map keyed by id string. `reap_exited` shows
  how a stamp is written (`record.exited_unix = Some(now)`).
- `src/daemon/executor/knowledge/shell.rs`: `list_shells` collects ids per
  scope, calls `refresh_programs`, then `resolve_scope` → rows →
  `format_shell_rows`. Filter the rows (or the ids) there.
- `src/cli/commands/top_load.rs`: the `Response::ShellList { shells }` arm
  that builds `ShellsPanel::Rows`. `ShellInfo.exited_unix` is on each row.
- Tests:
  - `src/daemon/executor/knowledge/shell_lifecycle_tests.rs`:
    `close_shell_ends_the_callers_shell`,
    `last_detach_closes_the_sessions_shells_after_the_grace`, and the
    `detach_close_spawn` helper.
  - `src/daemon/executor/knowledge/shell_tests.rs`: `has_record`, the `row`
    helper, and the `NOW` constant.

**Edit order.** Each step ends at a green `cargo build`.

1. `shell_life.rs`: the observed-gone branch stamps `exited_unix`. Either
   `closed_by_observed_end` takes `reg`, or it returns whether it saw the
   host gone and `close_shell_with` stamps. **This is your first edit, and
   it lands within your first 15 turns.**
2. `shell.rs`: `list_shells` drops rows with `exited_unix` set.
3. `top_load.rs`: the Shells panel drops the same rows.
4. Add the tests below. Then run the e2e. If an existing test asserted that
   `list_shells` (the tool, not the formatter) shows an `[exited]` row,
   change only that expectation and name the test in the Update Log.

## Test plan

- `close_shell_stamps_the_record_exited_once_the_host_is_gone`
  (`shell_lifecycle_tests.rs`): after `close_shell_with` returns its
  "Closed shell" sentence, the record still exists and its `exited_unix` is
  `Some`.
- `last_detach_marks_the_closed_shells_exited`: the detach-grace analogue.
  After `close_after_detach` closes a session's shell, its record is
  stamped exited, and a `dedicated` shell of the same session is not. This
  may extend `last_detach_closes_the_sessions_shells_after_the_grace`
  instead of adding a test, but the name above must exist and pass.
- `list_shells_omits_exited_shells` (`shell_tests.rs`): a registry holding
  one live and one exited record lists only the live one, under `mine` and
  under `all`.
- `top_shells_panel_omits_exited_rows` (the top loader's tests, or
  `top_tui_tests.rs`): a `ShellList` with one live and one exited
  `ShellInfo` gives a panel with only the live row.
- `close_shell_does_not_stamp_a_lingering_host`: if the host still holds
  its lock when the observation window ends, `exited_unix` stays `None`.
  A test seam that injects the liveness probe or the window length is
  authorized if one is needed.

## Acceptance criteria

- [ ] All five Test-plan tests pass, as printed by the e2e. None exists
      today.
- [ ] Every existing `close_shell_*`, `detach_close*`, `last_detach*`,
      `reap_idle*` and `formatter_*` test still passes. This includes
      `close_shell_ends_the_callers_shell`'s "must not remove the record".
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok.
      `build_warnings=0`, clippy is clean, `fmt_exit=0`, `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test --lib close_shell 2>&1 | grep -E '^test |^test result'
cargo test --lib last_detach 2>&1 | grep -E '^test |^test result'
cargo test --lib omits_exited 2>&1 | grep -E '^test |^test result'
cargo test --lib detach_close 2>&1 | grep -E '^test result'
cargo test --lib reap_idle 2>&1 | grep -E '^test result'
cargo test --lib formatter_ 2>&1 | grep -E '^test result'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

The architect re-checks the live clause (`list_shells` after a real `chat`
exit) at milestone close.

## Authorizations

- **Edit:** `src/daemon/executor/knowledge/shell_life.rs`,
  `src/daemon/executor/knowledge/shell.rs`,
  `src/cli/commands/top_load.rs`, and the tests in
  `shell_lifecycle_tests.rs`, `shell_tests.rs`, `top_tui_tests.rs` (or the
  test module beside `top_load.rs`). For an existing test, change
  expectations only, per edit order step 4.
- **Docs:** CLAUDE.md's `list_shells` row and README.md's `list_shells`
  mention may gain "exited shells are omitted". Change nothing else there.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, raise its entry and name it in Findings. Don't split code to fit.
- **Scratch files** go under `.rexymcp/output/`.

## Out of scope

- Removing a record on close. The PE chose stamp + hide.
- `reap_exited`'s hourly cadence and `exited_retention_secs`.
- `/shells`, the prompt's shells context block, and `read_shell*` on an
  exited id.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-13 00:24 (started, executor claude-sonnet-4-5-20250929)

Status flipped to in-progress. Reading `shell_life.rs`, `registry.rs`, `shell.rs`,
`top_load.rs` and the two test modules, then implementing the stamp-and-hide per
the Spec's edit order.

### Update — 2026-11-13 00:45 (end-to-end verification)

Ran the phase's E2E block mechanically, redirecting all output to
`.rexymcp/output/e2e-21e.log` and pasting it below verbatim:

```
test daemon::executor::knowledge::shell::tests::lifecycle::close_shell_does_not_stamp_a_lingering_host ... ok
test daemon::executor::knowledge::shell::tests::lifecycle::close_shell_refuses_another_owners_shell ... ok
test daemon::executor::knowledge::shell::tests::lifecycle::close_shell_stamps_the_record_exited_once_the_host_is_gone ... ok
test daemon::executor::knowledge::shell::tests::lifecycle::close_shell_unknown_id_is_an_error ... ok
test daemon::executor::knowledge::shell::tests::lifecycle::close_shell_ends_the_callers_shell ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 2806 filtered out; finished in 1.75s
test daemon::executor::knowledge::shell::tests::lifecycle::last_detach_closes_the_sessions_shells_after_the_grace ... ok
test daemon::executor::knowledge::shell::tests::lifecycle::last_detach_marks_the_closed_shells_exited ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 2809 filtered out; finished in 0.37s
test cli::commands::top::load::tests::shells_panel_omits_exited_rows ... ok
test daemon::executor::knowledge::shell::tests::list_shells_omits_exited_shells ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 2809 filtered out; finished in 0.00s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 2808 filtered out; finished in 0.37s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 2802 filtered out; finished in 0.28s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 2809 filtered out; finished in 0.00s
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.23s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.46s
fmt_exit=0
passed=2893 failed=0
```

All five Test-plan tests pass as printed (`close_shell_stamps_the_record_exited_once_the_host_is_gone`,
`last_detach_marks_the_closed_shells_exited`, `list_shells_omits_exited_shells`,
`shells_panel_omits_exited_rows`, `close_shell_does_not_stamp_a_lingering_host`);
every `close_shell_*`, `detach_close*`, `last_detach*`, `reap_idle*` and
`formatter_*` test still passes, including `close_shell_ends_the_callers_shell`'s
"must not remove the record". Hygiene 16, doc_truth 19, bug_tracker 6,
lock_invariant 2 — all ok. `build_warnings=0`, clippy clean, `fmt_exit=0`,
full suite `passed=2893 failed=0`.
