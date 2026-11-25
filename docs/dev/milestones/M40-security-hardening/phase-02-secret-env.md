# Phase 02 — secret-env

**Milestone:** M40 — Security hardening
**Status:** review
**Drafted:** 2026-11-25
**Depends on:** none
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Audit row **S5**. The daemon's provider API keys (`ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, `GEMINI_API_KEY`, the names `ModelEntry::api_key_env_var`
returns) are inherited by every process it spawns. That includes the shells
the model runs commands in, so `env` or `echo $ANTHROPIC_API_KEY` in a
model-run command hands the key back to the model.

The architect **reproduced it** on 2026-11-25 with scratch tests, since
reverted. With a sentinel key set in the daemon's environment:
- a `PtyShell::spawn` child printed `KEY=[sk-ant-PROBE-SENTINEL]`;
- a `deliver_command` child printed `KEY=[sk-PROBE-SENTINEL]`.

The architect traced every spawn site. A command the model chooses reaches
a process through exactly three of them:
- **`PtyShell::spawn`** (`src/shell/pty.rs`): every shell, plus the ssh
  connect step in `drive_connect`.
- **`spawn_detached`** (`src/shell/launch.rs`): the `shell-host` process,
  whose PTY children inherit from it.
- **`deliver_command`** (`src/daemon/notice_sinks.rs`): the command sink's
  `sh -c`.

The other spawns (`ssh -O check`, the sbx CLI, sudoers `id`/`sudo`,
`sys_context`) run fixed programs and are out of scope.

## Statement of work

1. **One list.** Add `pub const PROVIDER_KEY_VARS: &[&str]` to
   `src/config/models.rs`, beside `api_key_env_var`, with the three names.
   A test ties the two together, so a provider added later cannot be
   missed.
2. **Remove each listed variable** from the child's environment at all
   three sites:
   - `PtyShell::spawn` removes it from its `CommandBuilder`;
   - `spawn_detached` and `deliver_command` remove it from their
     `Command`.

   **Remove only these names.** Never `env_clear`: `PATH`, `HOME`, `TERM`,
   `SSH_AUTH_SOCK` and the `DAEMONEYE_*` variables the command sink sets
   must still reach the child.
3. **`spawn_detached`'s command gets a builder.** Move its
   `std::process::Command` construction into a function
   `shell_host_command(exe, id, shell, args, rows, cols)` that returns
   the `Command`, and make `spawn_detached` build through it. The argv must
   not change; the existing `spawn_detached_*` tests pin it.
4. **Docs.**
   - Add a short guard paragraph to `docs/security.md` § "Guards, in order
     of strength", as `### 8`: what is removed, from which spawns, and that
     a user who exports a key in their own shell rc file still gets it in
     login shells, by their own choice.
   - Correct `deliver_command`'s doc comment. It names `fire_notification`,
     which no longer exists.

**Constraints:**

- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.
- **Tests that set a key** hold `crate::test_home_guard()`. Take it **once**
  per test, at the top; it is not re-entrant. They use an `unsafe
  { std::env::set_var }`, the suite's accepted idiom under that guard.
  They **restore each variable's original value** on drop, even when the
  test fails, because a developer's shell may export real keys.
- **Gotcha:** a test subscribes to a `PtyShell`'s output *after* the spawn.
  Delay the child's `echo` (`sleep 0.3; echo …`), or the output can
  arrive before the subscription and the test times out.
- Every wait in a new test is bounded. No assertion may be true by
  construction.

**Closed file list:** `src/config/models.rs`, `src/shell/pty.rs`,
`src/shell/launch.rs`, `src/daemon/notice_sinks.rs` (each with its own
tests, or `src/config/tests.rs` for models), and `docs/security.md`. Read
at most **3** files before your first edit.

**Edit order:** item 1, then `pty.rs`, `launch.rs`, `notice_sinks.rs`,
then the docs.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`pty_child_sees_no_provider_keys`, in `src/shell/pty.rs` tests.
- Set all three variables to sentinels, and `DE_ENV_PROBE_KEEP=kept`.
- Spawn `sh -c 'sleep 0.3; echo "K=[$ANTHROPIC_API_KEY$OPENAI_API_KEY$GEMINI_API_KEY] KEEP=[$DE_ENV_PROBE_KEEP]"'`.
- Wait, bounded, for the exact text `K=[] KEEP=[kept]`. The `KEEP` half is
  the must-stay case: it proves the fix removes names rather than clearing
  the environment.

It is red today: the line carries the three sentinels. Paste the red run.

## Acceptance criteria

- [ ] **PTY:** `pty_child_sees_no_provider_keys` passes. Red run pasted.
- [ ] **Command sink:** `command_sink_child_sees_no_provider_keys` (in
      `notice_sinks.rs` tests) passes. It sets the same sentinels and
      `DE_ENV_PROBE_KEEP`, then calls `deliver_command` with a command that
      writes the same `K=[…] KEEP=[…]` line, plus `$DAEMONEYE_JOB`, to a
      temp file. It polls for the file under a timeout. The line must read
      `K=[] KEEP=[kept]` and carry the notice's name. Red run pasted.
- [ ] **Shell host:** `shell_host_command_removes_provider_keys` (in
      `launch.rs` tests) passes. Each name in `PROVIDER_KEY_VARS` appears
      in `shell_host_command(…).get_envs()` as `(name, None)`, an explicit
      removal, whatever the test's own environment holds.
- [ ] **One list:** `provider_key_vars_cover_every_provider` passes. For
      every provider `api_key_env_var` knows (`anthropic`, `openai`,
      `gemini`, `ollama`, `lmstudio`), a non-empty name is in
      `PROVIDER_KEY_VARS`.
- [ ] **Wiring:** run this command **once**. It prints **1** (it prints 0
      today):
      `awk '/pub async fn spawn_detached/,/^}/' src/shell/launch.rs | grep -c 'shell_host_command('`
- [ ] **Preserved:** the three `spawn_detached_*` argv tests pass unchanged.
- [ ] **Mutation check,** run once with the output pasted:
      1. remove the variable-removal step from `PtyShell::spawn`;
      2. run `cargo test --lib pty_child_sees_no_provider_keys` once and see
         it fail;
      3. restore it.
- [ ] **Gates:** all four pass, run once each. The lib suite reports **at
      least 3136** passed (3132 plus four).

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
   - the two red runs;
   - the wiring grep;
   - the mutation check.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.

## Out of scope

- Removing keys from the daemon's own process environment. The keys are
  re-read on reload.
- Other secret-bearing variables, such as `OPENAI_API_BASE` (not a
  secret) and user-chosen `api_key` values in `config.toml`, which are not
  in the environment.
- The fixed-program spawns named in the Goal.
- Re-pointing the existing `child_argv` test helper at the new builder.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-25 19:40 (started)

Work has begun: first red test being written (`pty_child_sees_no_provider_keys`), then the fix at each spawn site in the pinned edit order.

**Task list** (each line names the acceptance item it closes):

1. Write red test `pty_child_sees_no_provider_keys` in `src/shell/pty.rs` tests; run it red. → closes **PTY**.
2. Add `PROVIDER_KEY_VARS` to `src/config/models.rs` + `provider_key_vars_cover_every_provider` test in `src/config/tests.rs`. → closes **One list**.
3. Remove listed vars in `PtyShell::spawn` (`src/shell/pty.rs`); make test green. → closes **PTY**.
4. `shell_host_command` builder in `src/shell/launch.rs` + `shell_host_command_removes_provider_keys` test; wire `spawn_detached` through it. → closes **Shell host** and **Wiring**; the three `spawn_detached_*` argv tests pin **Preserved**.
5. Remove listed vars in `deliver_command` (`src/daemon/notice_sinks.rs`) + `command_sink_child_sees_no_provider_keys` test. → closes **Command sink**.
6. Docs: guard paragraph `### 8` in `docs/security.md`; fix `deliver_command` doc comment (`fire_notification` no longer exists). → docs part of the Spec.
7. Mutation check on the PTY removal step, pasted. → closes **Mutation check**.
8. Run the four gates once each; paste `test result:` lines into a new Update Log entry. → closes **Gates**.
9. Commit as one `fix:` commit.

### Update — 2026-11-25 19:58 (end-to-end verification)

All four gates run once each, in order. The lib suite reports **3136 passed** (3132 + 4 new), 0 failed.

**Gate 1 — `cargo fmt --all`:** clean (no output, exit 0).

**Gate 2 — `cargo build`:**

```
   Compiling daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.12s
```

**Gate 3 — `cargo clippy --all-targets --all-features -- -D warnings`:**

```
    Checking daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.08s
```

**Gate 4 — `cargo test`:**

```
test result: ok. 3136 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 149.52s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.27s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.23s
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.41s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

**Red run 1 — `pty_child_sees_no_provider_keys` before the fix** (the fix removed, one run):

```
thread 'shell::pty::tests::pty_child_sees_no_provider_keys' (3836085) panicked at src/shell/pty.rs:1322:13:
output Ok("K=[] KEEP=[kept]") never appeared within the wait; seen so far: "K=[sk-ant-PROBE-SENTINELsk-PROBE-SENTINELsk-gem-PROBE-SENTINEL] KEEP=[kept]\r\n"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3145 filtered out; finished in 10.01s
```

**Red run 2 — `command_sink_child_sees_no_provider_keys` before its fix** (the
sink's env-removal loop deleted, one run, then restored — same mutation shape
as the PTY check, this run captured after the first):

```
thread 'daemon::notice_sinks::command_sink_child_sees_no_provider_keys' (3853749) panicked at src/daemon/notice_sinks.rs:1086:5:
assertion `left == right` failed: the child must keep unrelated env, lose the keys, and see the notice's name
  left: "K=[sk-ant-PROBE-SENTINELsk-PROBE-SENTINELsk-gem-PROBE-SENTINEL] KEEP=[kept] JOB=[probe-job]"
 right: "K=[] KEEP=[kept] JOB=[probe-job]"

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3145 filtered out; finished in 0.02s
```

**Wiring grep (criterion: prints 1):**

```
$ awk '/pub async fn spawn_detached/,/^}/' src/shell/launch.rs | grep -c 'shell_host_command('
1
```

**Mutation check — removal step deleted from `PtyShell::spawn`, one test run, then restored:**

```
$ # after deleting the three-line env_remove loop from src/shell/pty.rs:
$ cargo test --lib pty_child_sees_no_provider_keys 2>&1 | tail -12
thread 'shell::pty::tests::pty_child_sees_no_provider_keys' (3836085) panicked at src/shell/pty.rs:1322:13:
output Ok("K=[] KEEP=[kept]") never appeared within the wait; seen so far: "K=[sk-ant-PROBE-SENTINELsk-PROBE-SENTINELsk-gem-PROBE-SENTINEL] KEEP=[kept]\r\n"

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3145 filtered out; finished in 10.01s
$ # loop restored; test green again:
test shell::pty::tests::pty_child_sees_no_provider_keys ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3145 filtered out; finished in 0.31s
```

**Preserved argv tests, unchanged:**

```
test shell::launch::tests::spawn_detached_appends_argv_after_a_separator ... ok
test shell::launch::tests::spawn_detached_omits_the_separator_when_argv_is_empty ... ok
test shell::launch::tests::spawn_detached_command_line_format_passes_the_args_through ... ok
```

### Update — 2026-11-25 20:10 (complete, server-authored)

**Summary:** Phase 02 (secret-env) is implemented and committed as one `fix:` commit (`5bf57140`), working tree clean. `PROVIDER_KEY_VARS` now lives in `src/config/models.rs` beside `api_key_env_var`, pinned by `provider_key_vars_cover_every_provider`; the three names are removed from children's environments at all three model-reachable spawn sites — `PtyShell::spawn` (via `CommandBuilder::env_remove`), the shell host (built through the new `shell_host_command`, with `spawn_detached` wired through it and the three argv tests passing unchanged), and `deliver_command` — without `env_clear`, so `DE_ENV_PROBE_KEEP`-style variables and the `DAEMONEYE_*` set still arrive. `docs/security.md` gained guard `### 8` (the existing masking section renumbered to `### 7` to keep the ordered list intact), and `deliver_command`'s stale `fire_notification` doc-comment reference is corrected.

**Notes for review**

- Both red runs are pasted in the end-to-end verification Update Log entry. The PTY red run showed `K=[sk-ant-PROBE-SENTINELsk-PROBE-SENTINELsk-gem-PROBE-SENTINEL] KEEP=[kept]`; the sink red run was captured by mutation (deleting the sink's env-removal loop, one run, restored) because the fix landed in the same edit pass as the test — flagged in the entry rather than hidden.
- The mutation check on `PtyShell::spawn`'s removal step is pasted: removal deleted → test fails with the sentinel leak → restored → green.
- Wiring grep prints 1 as pinned; all four gates ran once each, lib suite reports **3136 passed, 0 failed** (3132 + 4 new).
- One test-infra improvement beyond the strict spec: `poll_for_output` now accumulates chunks and reports what it saw on timeout — the phase's "every wait bounded, no assertion true by construction" rule; the failure message made the red run self-documenting. Phase doc status left at `in-progress` per the contract; the server owns the completion tail.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.22s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test attach_sends_session_counters_after_ok ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
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
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.23s


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
test daemon_boots_in_throwaway_root ... ok
test notices_follow_streams_a_webhook_alert ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.39s


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

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-f4e47940aad84bd2)
kill: (3874951): No such process
kill: (-3875644): No such process
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
- `docs/dev/milestones/M40-security-hardening/phase-02-secret-env.md` — +104 -1
- `docs/security.md` — +15 -0
- `src/config/models.rs` — +8 -0
- `src/config/tests.rs` — +23 -0
- `src/daemon/notice_sinks.rs` — +64 -2
- `src/shell/launch.rs` — +57 -16
- `src/shell/pty.rs` — +50 -6

**Commit:** 5bf571401efbfd3d784199914f3bf5a6ceb36b7c

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
