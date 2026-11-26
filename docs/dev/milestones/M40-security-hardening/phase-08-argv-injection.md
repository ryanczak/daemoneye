# Phase 08 — argv-injection

**Milestone:** M40 — Security hardening
**Status:** done
**Drafted:** 2026-11-26
**Depends on:** 07 (done)
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Audit row **S11**: argv flag injection. A value the daemon passes as an
operand can be read as an **option** when it starts with `-`.

**Re-verified by the architect on 2026-11-26:**
- **`HostId::parse`** (`src/host.rs`) accepts `[A-Za-z0-9._-]+`, so
  `-oProxyCommand=x` passes as a host name.
- **The ssh argv builders** (`src/host.rs`: `ssh_argv`, `connect_argv`,
  `ghost_ssh_argv`, and `master_running` in `src/shell/session.rs`) put
  `entry.ssh` last, with no `--` before it. An `ssh = "-oProxyCommand=…"`
  in `[hosts.*]` becomes an ssh option that runs a local command.
- **The remote `read_file` filter** (`build_remote_read_cmd` in
  `src/daemon/executor/file_ops/read.rs`) appends
  `| grep -E '<pattern>'`. A pattern such as `-f/etc/shadow` or `--version`
  is a grep option, not a pattern.
- **`client_supports_rgb`** (`src/tmux/session.rs`) runs
  `infocmp -x <termname>`. The termname comes from the tmux client.

**Checked on this machine, 2026-11-26:**
- `ssh -G -- -oProxyCommand=evil` answers "hostname contains invalid
  characters": after `--`, ssh takes the value as a host name.
- `infocmp -x -- -V` answers `no match in terminfo database for terminal
  type "-V"`.
- `printf 'a\n-x\n' | grep -E -e '-x'` prints `-x`.

So `--` (for ssh and infocmp) and `-e` (for grep) are each the whole fix.

## Statement of work

1. **`HostId::parse`** refuses a name whose first character is `-`. The
   error names the leading `-`. `"-"` alone, `"-web"` and `"--x"` are
   refused. `"web-1"`, `"a-"`, `"db.lan"` and `"local"` are unchanged.
2. **ssh argv.** `ssh_argv`, `connect_argv`, `ghost_ssh_argv` and
   `master_running` each emit `"--"` **immediately before** `entry.ssh`.
   `entry.ssh` stays the last element and stays one element.
   - **GOTCHA:** `ghost_ssh_argv` inserts `-o BatchMode=yes` at
     `argv.len() - 1`. With `--` in place, that position is *after* `--`,
     where ssh would read the option as the host name. Insert before the
     `--`.
   - **GOTCHA:** `src/daemon/executor/shell_exec_hosted_tests.rs`
     (`ssh_argv` + `argv.insert(dest, …)`) makes the same insertion. Fix it
     the same way.
   - Update the doc comments that say "the destination is last" to say "`--`
     then the destination, last".
3. **Remote grep.** `build_remote_read_cmd` emits `| grep -E -e '<pattern>'`.
   The quoting via `sq_escape` is unchanged. Update
   `build_remote_read_cmd_is_unchanged` in `read_tests.rs` to pin the new
   string.
4. **infocmp.** Move the argv into a small pure function, for example
   `fn infocmp_args(termname: &str) -> [&str; 3]` returning
   `["-x", "--", termname]`. `client_supports_rgb` calls it. Test it.
5. **Constraints.**
   - No other behaviour changes. `--` is accepted by every ssh this daemon
     supports (OpenSSH uses getopt).
   - **No existing test is removed.** Tests that pin the old argv are
     updated to the new one.

**Closed file list:**
- `src/host.rs`, `src/shell/session.rs`;
- `src/daemon/executor/shell_exec_hosted_tests.rs`;
- `src/daemon/executor/file_ops/read.rs`,
  `src/daemon/executor/file_ops/read_tests.rs`;
- `src/tmux/session.rs`;
- any other `*_tests.rs` whose assertion on the **old** argv fails after
  item 2. Name each one in the Update Log.

**Edit order.** Each step ends with `cargo build --tests`.
1. **Your first tool call is an edit to `src/host.rs`:** the `HostId::parse`
   check and its test.
2. The ssh argv builders in `src/host.rs` and their tests, then
   `master_running` and the hosted-test insertion.
3. The grep in `read.rs`, and `read_tests.rs`.
4. `infocmp_args` and its test.

**Read limit.** Read at most **3** files before your first edit. After
**5 reads in a row**, your next call must be an edit or a `cargo` command.

**Budget.** You have **1200 turns**; use them efficiently.

## First red test

`host_id_refuses_a_leading_dash`, in `src/host.rs`'s tests.
- **Refused:** `-oProxyCommand=x`, `-`, `-web` and `--x` each give `Err`.
- **Accepted:** `web-1` and `a-` each give `Ok(HostId::Named(_))`.

It is red today: the leading-dash names parse. Paste the red run.

## Acceptance criteria

- [ ] **Leading dash refused:** `host_id_refuses_a_leading_dash` passes.
      Paste the red run.
- [ ] **ssh argv ends `--`, destination:**
      `ssh_argvs_end_with_double_dash_then_destination` passes.
      - **Call:** each of `ssh_argv`, `connect_argv(batch: false)`,
        `connect_argv(batch: true)` and `ghost_ssh_argv`, with
        `ssh = "-oProxyCommand=evil"`.
      - **Asserts:** the last two elements are `"--"` and
        `"-oProxyCommand=evil"`, and `"BatchMode=yes"`, where present,
        sits before the `--`.
- [ ] **Existing argv tests:** the `ssh_argv`, `connect_argv` and
      `ghost_ssh_argv` tests in `src/host.rs` pass, updated to the new
      argv. `ghost_ssh_argv_sets_batch_mode_before_the_destination` still
      asserts that `BatchMode` comes before the destination.
- [ ] **grep takes `-e`:** run this once. It prints **1** (0 today):
      `grep -c "grep -E -e '" src/daemon/executor/file_ops/read.rs`
      `build_remote_read_cmd_is_unchanged` passes with the new string.
- [ ] **infocmp takes `--`:** `infocmp_args_end_options_before_the_termname`
      passes, for termname `-V`: the args are `["-x", "--", "-V"]`.
- [ ] **Mutation run, pasted as output:**
      1. Remove the `"--"` push from `ssh_argv` only.
      2. Run `cargo test --lib ssh_argvs_end_with_double_dash_then_destination`
         once. It must **fail**.
      3. Restore by undoing that one edit, never by copying a backup file.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff e37513ed -- src | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Gates:** all four pass, each run once.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** follow the edit order, writing each red test before its
   fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red run and the mutation run, **as output, not summarized**;
   - the two greps.

   Then commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.
- You may add the private helper `infocmp_args` in `src/tmux/session.rs`.

## Out of scope

- **Validating `[hosts.*] ssh` values at config load.** `--` makes a
  leading-dash value inert, and ssh rejects it as a host name.
- **The `"-O"` argv in test files.** Those are test teardown; leave them
  alone unless a test fails.
- **`sed -n` in `build_remote_read_cmd`.** The path is already required
  to be absolute, so it starts with `/`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-26 19:40 (started)

**Executor started phase 08.** Status flipped to `in-progress` here and in the
milestone README phase table.

**Task breakdown** (each closes the acceptance item named):

1. Write the red test `host_id_refuses_a_leading_dash` in `src/host.rs`'s
   tests and run it red — closes "Leading dash refused".
2. Add the leading-`-` check to `HostId::parse`; re-run the test green —
   closes "Leading dash refused".
3. `ssh_argv`, `connect_argv`, `ghost_ssh_argv` (in `src/host.rs`) and
   `master_running` (in `src/shell/session.rs`) each emit `"--"` immediately
   before `entry.ssh`; `entry.ssh` stays last and one element. Insert
   `BatchMode` before the `--` in `ghost_ssh_argv`; fix the same insertion in
   `src/daemon/executor/shell_exec_hosted_tests.rs`. Update doc comments that
   say "the destination is last". Update existing argv tests — closes
   "ssh argv ends `--`, destination" and "Existing argv tests".
4. Write `ssh_argvs_end_with_double_dash_then_destination` — closes
   "ssh argv ends `--`, destination".
5. `build_remote_read_cmd` emits `| grep -E -e '<pattern>'`; update
   `build_remote_read_cmd_is_unchanged` in `read_tests.rs` — closes
   "grep takes `-e`".
6. Add `infocmp_args` to `src/tmux/session.rs` returning
   `["-x", "--", termname]`; `client_supports_rgb` calls it; add
   `infocmp_args_end_options_before_the_termname` — closes
   "infocmp takes `--`".
7. Mutation run on `ssh_argv`'s `--` push (fail, restore) — closes
   "Mutation run".
8. Grep checks: `grep -c "grep -E -e '"` on `read.rs` prints 1;
   `git diff e37513ed -- src | grep -cE '^-\s*#\[(tokio::)?test\]'` prints 0 —
   closes "No test removed".
9. Run the four gates (`cargo fmt --all`, `cargo build`,
   `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo test`), capture output — closes "Gates".
10. End-to-end verification entry; commit as one `fix:` commit.

### Update — 2026-11-26 19:55 (end-to-end verification)

**Red run** (before the `HostId::parse` fix):

```
---- host::tests::host_id_refuses_a_leading_dash stdout ----

thread 'host::tests::host_id_refuses_a_leading_dash' (385424) panicked at src/host.rs:357:13:
error for "-oProxyCommand=x" must name the leading dash, got "host name \"-oProxyCommand=x\" contains a character outside [A-Za-z0-9._-]; expected [A-Za-z0-9._-]+, 1-64 chars"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    host::tests::host_id_refuses_a_leading_dash

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3187 filtered out; finished in 0.00s
```

**Mutation run** — removed the `"--"` push from `ssh_argv` only, then:

```
$ cargo test --lib ssh_argvs_end_with_double_dash_then_destination

failures:
    host::tests::ssh_argvs_end_with_double_dash_then_destination

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3189 filtered out; finished in 0.00s
```

Restored by re-adding the one line (verified green afterwards). Positive
control: the mutation makes the pinned test fail, so the test is live.

**Grep — grep takes `-e`:**

```
$ grep -c "grep -E -e '" src/daemon/executor/file_ops/read.rs
1
```

**Grep — no test removed:**

```
$ git diff e37513ed -- src | grep -cE '^-\s*#\[(tokio::)?test\]'
0
```

**Existing argv tests updated, not removed:** `ssh_argv_puts_the_destination_last`
and `ghost_ssh_argv_sets_batch_mode_before_the_destination` in `src/host.rs`
now pin `--` immediately before the destination; the BatchMode pair still
comes before the destination. Files whose argv assertions changed:
`src/host.rs`, `src/daemon/executor/shell_exec_hosted_tests.rs`.

**Live ssh/infocmp behaviour** (checked by the architect in the phase doc):
`ssh -G -- -oProxyCommand=evil` → "hostname contains invalid characters";
`infocmp -x -- -V` → `no match ... for terminal type "-V"`;
`grep -E -e '-x'` matches a line `-x`.

**Gates:**

```
$ cargo fmt --all && echo FMT_OK
FMT_OK

$ cargo build 2>&1 | tail -3
   Compiling daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.44s

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
    Checking daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.36s

$ cargo test 2>&1 | grep -E "^test result:"
test result: ok. 3180 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 152.61s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.33s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.24s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

### Update — 2026-11-26 20:00 (complete, server-authored)

**Summary:** Phase 08 (argv-injection, row S11) is implemented and committed as one `fix:` commit; the phase doc status remains `in-progress` for the server's completion tail. `HostId::parse` now refuses names with a leading `-` (error names the leading dash; `web-1`, `a-`, `db.lan`, `local` unchanged); `ssh_argv`, `connect_argv`, `ghost_ssh_argv`, and `master_running` each emit `--` immediately before `entry.ssh`, with `BatchMode` inserted before the `--` in `ghost_ssh_argv` and the mirrored insertion in `shell_exec_hosted_tests.rs`; `build_remote_read_cmd` emits `grep -E -e '<pattern>'`; and the new private `infocmp_args` returns `["-x", "--", termname]`. New tests `host_id_refuses_a_leading_dash`, `ssh_argvs_end_with_double_dash_then_destination`, and `infocmp_args_end_options_before_the_termname` pass; the two existing argv tests were updated to the new argv (none removed — the no-test-removed grep prints 0).

**Notes for review:**
- The red run, the mutation run (removing the `--` push from `ssh_argv` alone fails the pinned test; restored by re-adding the one line), both greps, and all four gate outputs are pasted verbatim in the end-to-end verification Update Log entry.
- Deviation worth noting: the spec sketched `infocmp_args` returning `[&'static str; 3]`, but the termname is runtime data, so the actual signature is `fn infocmp_args(termname: &str) -> [&str; 3]` — same shape, no static-lifetime contortion.
- During implementation the server-side verifier caught one intermediate state (missing `infocmp_args` before the call site was built); fixed immediately and `cargo build --tests` went green.
- One incidental drift fix: an earlier patch briefly mangled a `fn sess(...)` signature line in `src/tmux/session.rs` test module; corrected in the same pass (no behavior change, `cargo test` green).
- Final `cargo test`: 3180 lib + all integration suites pass, 0 failures; fmt/clippy (`-D warnings`) clean.

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
t ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test attach_sends_session_counters_after_ok ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
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
test webhook_alert_to_event_log ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.24s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test notices_follow_streams_a_webhook_alert ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.41s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-f4e47940aad84bd2)
kill: (426834): No such process
kill: (-427534): No such process
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
- `docs/dev/milestones/M40-security-hardening/phase-08-argv-injection.md` — +121 -1
- `src/daemon/executor/file_ops/read.rs` — +1 -1
- `src/daemon/executor/file_ops/read_tests.rs` — +1 -1
- `src/daemon/executor/shell_exec_hosted_tests.rs` — +3 -3
- `src/host.rs` — +99 -18
- `src/shell/session.rs` — +3 -1
- `src/tmux/session.rs` — +19 -1

**Commit:** 6dceef9e71106018ac03af5990b70d4d1504d3fd

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** GLM-5.3-Flash
- **Scope deviations:** none. `infocmp_args` borrows its termname
  (`fn infocmp_args(termname: &str) -> [&str; 3]`), which is what the spec
  sketched.
- **Reviewer checks:** four gates green (lib 3180 = 3177 + the three new
  tests, doc_truth 19, hygiene 16); grep `-e` prints 1; no test removed
  (0); no new `unwrap`/`expect`/`#[allow]`. `ghost_ssh_argv` and the
  hosted pinky test both insert `BatchMode` before the `--`. The pasted
  mutation (drop `ssh_argv`'s `--`) fails
  `ssh_argvs_end_with_double_dash_then_destination`, which asserts the last
  two elements — accepted as output.
- **Calibration:** architect slip — the Goal claimed `-oProxyCommand=x`
  parsed as a host name; `=` was already outside the charset, so it was
  refused before. The red run was still real (`-web`, `--x` parsed, and the
  test pins the error wording). Run a one-line check for each claimed
  bypass example at drafting. 93 turns, no stall, closed file list held.

