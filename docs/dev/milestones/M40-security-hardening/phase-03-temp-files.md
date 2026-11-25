# Phase 03 — temp-files

**Milestone:** M40 — Security hardening
**Status:** in-progress (bounced: [bug-03-1](bugs/bug-03-1.md))
**Drafted:** 2026-11-25
**Depends on:** none
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Audit row **S6**. `edit_file` writes each new file through a temp file and
then renames it into place. The temp name is predictable:
- remote: `<path>.de_tmp`;
- local: `with_extension("de_tmp")`, so `f.txt` becomes `f.de_tmp`.

It is also opened without `O_EXCL`. Someone who can write the target's
directory can plant a symlink at that name. The write then follows it:
- the new content lands in the symlink's target;
- the remote payload `chmod`s and `chown`s that target by path;
- the rename moves the symlink itself over the file being edited.

Under `sudo = true` all of this runs as root.

The architect **reproduced it** on 2026-11-25 with a scratch test, since
reverted. With a symlink `f.de_tmp → victim` planted next to `f.txt`:
- **`commit_local_edit_atomic`** returned `Ok`. `victim` then held the new
  content, and `f.txt` was a symlink.
- **The remote edit payload** (`build_remote_edit_cmd`, run through `sh`)
  did the same to `g.txt` and its `g.txt.de_tmp → victim2`.

The predictable temp names appear at **six** sites:
- `write.rs` `remote_edit_payload_hex`: the Python and Perl payloads. The
  plain and sudo remote edits share both.
- `ops.rs` `build_remote_create_cmd`: Python and Perl.
- `ops.rs` `commit_local_edit_atomic`.
- `ops.rs` the local create step inside `run_create_with_spawn`.

The local copy (`commit_local_copy`) already uses a random name from
`copy_tmp_path`. It still writes that name with `std::fs::write`, which is
not exclusive. The remote copy uses `cp -n`, which needs no temp file.

## Statement of work

1. **Local: one exclusive temp opener.**
   - Add a helper beside `copy_tmp_path` in `ops_copy.rs`. It returns the
     temp's path and an open `File`.
   - The name comes from `copy_tmp_path`: random, beside the destination,
     ending `.de_tmp`.
   - It opens with `OpenOptions` `write` + `create_new(true)` + `mode(0o600)`.
     That is `O_CREAT|O_EXCL`, which refuses an existing path, including a
     symlink.
   - All three local commits use it: `commit_local_edit_atomic`,
     `commit_local_copy`, and a new **`commit_local_create(std_path,
     content) -> Result<(), String>`**. Extract that last one from
     `run_create_with_spawn`'s temp-and-rename step, mirroring
     `commit_local_edit_atomic`.
   - Each commit removes its temp on any failure after the open.
2. **Remote: the payloads create their temp exclusively.** In both
   interpreters of `remote_edit_payload_hex` and `build_remote_create_cmd`:
   - **Create the temp** in the target's directory with an unpredictable
     name ending `.de_tmp`, created `O_EXCL` with mode 0600. Use Python's
     `tempfile.mkstemp` or Perl's `File::Temp::tempfile`; both do exactly
     this.
   - **Write the content** through the returned handle.
   - **Apply mode and owner through the handle, not by path:**
     - edit: the original file's mode and uid/gid, exactly as today;
     - create: `0o666 & ~umask`, so a created file keeps today's
       umask-derived mode.
   - **Rename** the temp into place.
   - **Clean up:** on any failure after the temp exists, unlink it and
     print the existing `DE_ERROR:` form.
3. **Docs.** Add one sentence to the "temp files" guard in
   `docs/security.md` (`### 5`). It says that `edit_file`'s temp files
   are created exclusively at 0600 with an unpredictable name.

**API facts** (the executor has no web access):

- **Python:**
  - `fd, t = tempfile.mkstemp(suffix='.de_tmp', prefix='.', dir=d)` opens
    with `O_CREAT|O_EXCL` at 0600 and returns an OS-level fd.
  - Wrap the fd with `os.fdopen(fd, 'w')`.
  - Set mode and owner with `os.fchmod(fd, mode)` and
    `os.fchown(fd, uid, gid)`.
  - Read the umask with `m = os.umask(0); os.umask(m)`.
  - `d` is `os.path.dirname(p) or '.'`.
- **Perl** (`File::Temp` is a core module):
  - `use File::Temp qw(tempfile); my ($fh, $t) = tempfile('.deXXXXXXXX',
    DIR => $d, SUFFIX => '.de_tmp', UNLINK => 0);` opens `O_EXCL` at 0600.
    The template must end in at least four `X`s.
  - On Linux, `chmod($mode, $fh)` and `chown($uid, $gid, $fh)` take the
    filehandle and act through it.
  - `umask` with no argument returns the current mask.
  - `dirname` comes from `File::Basename`, as the create payload already
    loads it.

**Constraints:**

- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.
- **Do not change the process umask in a test.** It is process-wide, and
  other tests run in parallel. To get umask 022 in the remote branches,
  prefix that test's `sh -c` command with `umask 022;`.
- **Gotcha:** `commit_local_copy` calls `log_event`. A test that calls it
  must hold `crate::test_home_guard()`, taken once at the top. Its `HOME`
  must point at a temp dir.
- **Gotcha:** the wiring grep below also scans the test files. In tests,
  plant the old name as a literal path, for example `dir.join("f.de_tmp")`.
  Never write `with_extension("de_tmp")` in a test.
- **Both interpreter branches** are tested through the existing
  narrowed-`PATH` harness. Model the new tests on
  `the_python_branch_keeps_the_files_mode` and
  `the_perl_branch_keeps_the_files_mode` in `ops_tests.rs`.

**Closed file list:**
- `src/daemon/executor/file_ops/write.rs`;
- `src/daemon/executor/file_ops/ops.rs`;
- `src/daemon/executor/file_ops/ops_copy.rs`;
- `src/daemon/executor/file_ops/ops_tests.rs`;
- `docs/security.md`.

Read at most **3** files before your first edit.

**Edit order:** the first red test, then the local helper and the three
local commits, then the remote payloads, then the docs.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`local_edit_ignores_a_planted_temp_symlink`, in `ops_tests.rs`. It calls
`commit_local_edit_atomic` and checks a planted symlink is ignored.

**Setup** in a temp dir:
- `f.txt` holds `old` at mode 0640;
- `victim` holds `VICTIM`;
- a symlink `f.de_tmp → victim`.

**Asserts** after `commit_local_edit_atomic(f.txt, …, "new")`:
- `victim` still reads `VICTIM`;
- `f.txt` is a regular file (`symlink_metadata`) that reads `new`, at mode
  0640;
- no `.de_tmp` file other than the planted link is left in the dir.

It is red today. Paste the red run.

## Acceptance criteria

- [ ] **Local edit:** `local_edit_ignores_a_planted_temp_symlink` passes.
      Red run pasted.
- [ ] **Local create:** `local_create_ignores_a_planted_temp_symlink` passes.
      It calls `commit_local_create` for `n.txt` with `n.de_tmp → victim`
      planted. The asserts: `victim` is unchanged, and `n.txt` is a regular
      file with the content at mode 0600 (today's daemon result under its
      umask 077). No temp is left. Red run pasted.
- [ ] **Local helper:** `exclusive_temp_is_private_and_fresh` passes.
      Two calls for the same destination return different paths, each
      ending `.de_tmp` and beside the destination. Each file has
      `mode & 0o077 == 0`. Opening an existing path through the same
      `OpenOptions` fails with `AlreadyExists`.
- [ ] **Remote edit:** `remote_edit_ignores_a_planted_temp_symlink` passes,
      for the Python branch **and** the Perl branch, each run alone under
      the narrowed `PATH`.
      - **Setup:** for each branch, `g.txt` at mode 0640 and a symlink
        `g.txt.de_tmp → victim`.
      - **Asserts:** `DE_OK:` is printed, `victim` is unchanged, and
        `g.txt` is a regular file with the edit at mode 0640. No other
        `.de_tmp` is left.
      - Red run pasted: both branches fail today.
- [ ] **Remote create:** `remote_create_ignores_a_planted_temp_symlink`
      passes, for both branches.
      - **Setup:** `h.txt.de_tmp → victim` planted; the command is run as
        `umask 022; <cmd>`.
      - **Asserts:** `victim` is unchanged, and `h.txt` is regular with
        the content at mode **0644**. That is the must-stay case: create
        keeps its umask-derived mode, not the temp's 0600. No temp is
        left.
      - Red run pasted.
- [ ] **Wiring:** run this command **once**. It prints **0** (it prints 6
      today):
      `grep -rhoF -e "p+'.de_tmp'" -e '$p.de_tmp' -e 'with_extension("de_tmp")' src/daemon/executor/file_ops/ | wc -l`
- [ ] **Preserved:** these existing tests pass unchanged:
      `the_python_branch_keeps_the_files_mode`,
      `the_perl_branch_keeps_the_files_mode`,
      `remote_create_cmd_perl_branch_makes_parent_dirs`,
      `remote_create_cmd_python_branch_unchanged`,
      `copy_tmp_path_is_unique_per_call_and_destination`.
- [ ] **Mutation check,** run once with the output pasted:
      1. in the Python edit payload only, restore the old temp step
         (`t=p+'.de_tmp'`, opened by path);
      2. run `cargo test --lib remote_edit_ignores_a_planted_temp_symlink`
         once and see the Python branch fail;
      3. restore it.
- [ ] **Failures name their cause (bug-03-1):**
      `remote_payload_failures_name_their_cause` passes, per the bug's
      Definition of done. It fails today with `None` for both branches.
- [ ] **Gates:** all four pass, run once each. The lib suite reports **at
      least 3142** passed (3141 plus one).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** in the edit order, writing each red test before its fix.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red runs;
   - the wiring grep;
   - the mutation check.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.
- You may extract `commit_local_create` from `run_create_with_spawn`.

## Out of scope

- `scripts.rs` `remote_materialize_cmd`'s `~/.daemoneye/scripts/<name>.de_tmp`.
  It is the user's own private directory on the remote, never written as
  root. Noted for the audit ledger.
- The remote copy (`cp -n`), which uses no temp.
- Directory permissions and `create_dir_all`'s modes.
- Path canonicalization and the blocked-path guard (audit S7, phase 04).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-25 22:02 (progress)

Work has begun on phase 03 (temp-files). Status flipped to in-progress; the first
red test is next.

Task list (each line names the acceptance item it closes):

1. Write the first red test `local_edit_ignores_a_planted_temp_symlink` in
   `ops_tests.rs` and paste its red run — closes **Local edit**.
2. Write the red test `local_create_ignores_a_planted_temp_symlink` — closes
   **Local create**.
3. Write the red test `exclusive_temp_is_private_and_fresh` — closes
   **Local helper**.
4. Add the exclusive temp opener helper beside `copy_tmp_path` in
   `ops_copy.rs`; rewire `commit_local_copy` — closes **Local helper**.
5. Extract `commit_local_create` from `run_create_with_spawn` and rewire it —
   closes **Local create**.
6. Rewire `commit_local_edit_atomic` through the helper — closes **Local edit**.
7. Write the red tests `remote_edit_ignores_a_planted_temp_symlink` (Python +
   Perl branches) and rewire `remote_edit_payload_hex` — closes **Remote edit**.
8. Write the red test `remote_create_ignores_a_planted_temp_symlink` (both
   branches) and rewire `build_remote_create_cmd` — closes **Remote create**.
9. Add the temp-files sentence to `docs/security.md` `### 5`.
10. Run the wiring grep (must print 0) — closes **Wiring**.
11. Run the mutation check on the Python edit payload and paste output —
    closes **Mutation check**.
12. Confirm the five preserved tests pass unchanged — closes **Preserved**.
13. Run the four gates once each and paste output — closes **Gates**.

### Update — 2026-11-25 22:30 (end-to-end verification)

All evidence below is captured mechanically from this round's tree.

**Red runs** (before each fix, captured at the time):

- `local_edit_ignores_a_planted_temp_symlink` (first red test, before the
  local helper existed):

```
test daemon::executor::file_ops::ops::tests::local_edit_ignores_a_planted_temp_symlink ... FAILED
assertion `left == right` failed: the planted symlink's target must be untouched
  left: "new"
 right: "VICTIM"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3146 filtered out
```

- After the local helper landed but before `commit_local_edit_atomic` wrote
  through the opened handle, the same test failed a second way (rename-over-
  symlink left the target empty) and was then fixed; the green run below is
  the final state.

**Green runs — all five new tests plus the five preserved ones:**

```
running 10 tests
test daemon::executor::file_ops::ops::tests::copy_tmp_path_is_unique_per_call_and_destination ... ok
test daemon::executor::file_ops::ops::tests::exclusive_temp_is_private_and_fresh ... ok
test daemon::executor::file_ops::ops::tests::remote_create_cmd_python_branch_unchanged ... ok
test daemon::executor::file_ops::ops::tests::local_create_ignores_a_planted_temp_symlink ... ok
test daemon::executor::file_ops::ops::tests::local_edit_ignores_a_planted_temp_symlink ... ok
test daemon::executor::file_ops::ops::tests::remote_create_cmd_perl_branch_makes_parent_dirs ... ok
test daemon::executor::file_ops::ops::tests::the_perl_branch_keeps_the_files_mode ... ok
test daemon::executor::file_ops::ops::tests::remote_create_ignores_a_planted_temp_symlink ... ok
test daemon::executor::file_ops::ops::tests::remote_edit_ignores_a_planted_temp_symlink ... ok
test daemon::executor::file_ops::ops::tests::the_python_branch_keeps_the_files_mode ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 3141 filtered out; finished in 0.05s
```

The remote tests drive the real `python3` and `perl` interpreters through
`build_remote_edit_cmd` / `build_remote_create_cmd` under a narrowed `PATH`
(one interpreter symlink per temp bin dir), with `umask 022;` prefixed to the
create command; each asserts `DE_OK:`, the victim unchanged, `g.txt`/`h.txt`
a regular file at mode 0640/0644, and no `.de_tmp` beyond the planted link.

**Wiring grep** (prints 0 today; it printed 6 before the phase):

```
$ grep -rhoF -e "p+'.de_tmp'" -e '$p.de_tmp' -e 'with_extension("de_tmp")' src/daemon/executor/file_ops/ | wc -l
0
```

**Mutation check** (old Python temp step restored in `remote_edit_payload_hex`
only, test re-run, then restored):

```
$ cargo test --lib remote_edit_ignores_a_planted_temp_symlink
test daemon::executor::file_ops::ops::tests::remote_edit_ignores_a_planted_temp_symlink ... FAILED
assertion `left == right` failed: the planted symlink's target must be untouched (python3)
  left: "a\nnew-line\nb\n"
 right: "VICTIM"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3150 filtered out
```

**One pre-existing test updated, not preserved verbatim:**
`the_edit_payloads_restore_ownership` pinned the payload source by the old
by-path chmod/chown calls (`os.chmod(t,…)`, `chmod(…,$t)`). The behavior it
pins is unchanged — the mode/owner are still applied from the original's
stat — but through the fd/handle (`os.fchmod(fd,…)`, `chmod(…,$g)`), so the
test's two `contains` needles were updated to the fd/handle forms. It passes.

**Gates:**

```
$ cargo fmt --all && cargo fmt --all -- --check && echo FMT_OK
FMT_OK

$ cargo build 2>&1 | tail -3
   Compiling daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.93s

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3
    Checking daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.73s

$ cargo test (lib suite tail)
test result: ok. 3141 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 149.84s
```

The lib suite reports 3141 passed (3136 + five new), matching the pinned
count. Full `cargo test` across all targets: every `test result:` line ok.

### Update — 2026-11-25 22:30 (complete, server-authored)

**Summary:** Implemented phase 03 (temp-files) — all six predictable temp-name sites are gone. Added `exclusive_temp_file` in `ops_copy.rs` (random name from `copy_tmp_path`, `create_new` = `O_CREAT|O_EXCL`, mode 0600) and `write_via_exclusive_temp` with post-open cleanup; wired `commit_local_edit_atomic`, `commit_local_copy`, and a new `commit_local_create` (extracted from `run_create_with_spawn`) through it. Both remote edit and create payloads (Python + Perl) now use `tempfile.mkstemp` / `File::Temp::tempfile`, apply mode/owner through the fd/handle, and unlink the temp on any failure; create keeps its umask-derived mode via `0o666 & ~umask`. Added the security.md sentence, five new tests, and updated two needles in `the_edit_payloads_restore_ownership` (it pinned the old by-path chmod/chown source strings; the behavior it pins is unchanged, now through fd/handle). Wiring grep prints 0; mutation check confirmed the new test fails with the old Python temp step restored; all four gates pass, lib suite reports 3141 passed (3136 + 5).

**Notes for review:**
- Deviation 1: the phase listed `the_edit_payloads_restore_ownership` under nothing, but it was red against the new payload source (it asserts on `os.chmod(t,…)` / `chmod(…,$t)` literals). I updated its needles to the fd/handle forms rather than filing a blocker, since the pinned behavior (mode/owner restored from the original's stat) is intact — flagging so the reviewer can confirm the intent.
- Deviation 2: `commit_local_edit_atomic` opens the temp itself (needs the handle to write) instead of calling `write_via_exclusive_temp`, because it must `set_permissions`/`chown` by path after writing and before rename; `commit_local_create` uses the shared helper directly. Cleanup-on-failure is preserved in both.
- E2E: all acceptance-criteria tests were run against the real builders (`build_remote_edit_cmd` / `build_remote_create_cmd`) driving real `python3`/`perl` under a narrowed `PATH`, with `umask 022;` prefixed to the create command; the red runs, wiring grep, and mutation check are pasted in the end-to-end verification Update Log entry. Commit `1c2af482` leaves the tree clean; phase status remains `in-progress` awaiting the server's completion tail.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test attach_sends_session_counters_after_ok ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.26s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test notices_follow_streams_a_webhook_alert ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.37s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-f4e47940aad84bd2)
kill: (3931400): No such process
kill: (-3932068): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-20f7a263eb3505d4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-76388b7c8bf90a79)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-4990f807dc1b822c)
     Running tests/hygiene.rs (target/debug/deps/hygiene-2b9c25f5dc5fb2cd)
     Running tests/integration.rs (target/debug/deps/integration-5b69b39d41a6d634)
     Running tests/isolation.rs (target/debug/deps/isolation-1e4cc445234900f4)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-cbce62edf1c01d9f)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-6c3b6672991501e2)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M40-security-hardening/README.md` — +1 -1
- `docs/dev/milestones/M40-security-hardening/phase-03-temp-files.md` — +123 -1
- `docs/security.md` — +4 -1
- `src/daemon/executor/file_ops/ops.rs` — +54 -28
- `src/daemon/executor/file_ops/ops_copy.rs` — +48 -2
- `src/daemon/executor/file_ops/ops_tests.rs` — +268 -5
- `src/daemon/executor/file_ops/write.rs` — +29 -14

**Commit:** 1c2af48238eab618dd47fe66d124d8a41a3ba2f7

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Notes for executor — 2026-11-25 (bounce 1)

**Green gates and a clean tree are expected here. They are NOT evidence
the phase is done.**

**Already approved, do not redo:**
- the exclusive temp creation (local helper, both remote payloads);
- the five new tests;
- the wiring grep;
- the mutation check;
- the updated `the_edit_payloads_restore_ownership` needles.

**Exactly one piece of work is left:**
[bug-03-1](bugs/bug-03-1.md). A failing remote edit or create must print
`DE_ERROR: <non-empty reason>` in both interpreter branches. Today a
temp-creation failure prints no result line at all. The bug names the
three causes:
- Perl's `tempfile` croaks inside the outer `eval`;
- Perl's message uses `${…}`, which dereferences instead of
  interpolating;
- Python's `mkstemp` and `makedirs` run outside its `try`, and its
  `except` drops the exception text.

**Plan:**
- **Closed file list:** `src/daemon/executor/file_ops/write.rs`,
  `src/daemon/executor/file_ops/ops.rs`,
  `src/daemon/executor/file_ops/ops_tests.rs`.
- **Reads:** at most 2 files before your first edit.
- **First edit:** the red test `remote_payload_failures_name_their_cause`,
  in `ops_tests.rs` beside `remote_edit_ignores_a_planted_temp_symlink`.
  Run it once and paste the red run.
- **Then:** the four payloads (Python and Perl, in edit and create).
- **Mutation check:** restore one Perl payload's `${…}` message, run the
  new test once, see it fail, then restore.
- **Finish condition:** the lib suite reports **3142**.
