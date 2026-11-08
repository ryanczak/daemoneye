# Phase 17c — idle-reap

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-08
**Depends on:** phase 17b
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bug, size=m

## Goal

This phase fixes the last part of B11 (`FINDINGS.md`, major). Phase 17a
closes a chat session's shells after its last detach, and 17b re-adopts
shells under their owner after a restart. Some shells are still never
closed:

- shells adopted without an owner sidecar (`Owner::Adopted`), which no
  session may drive or close;
- shells re-adopted under a chat session that never re-attaches. 17a's
  detach grace starts only on a detach, so it never fires for them;
- any other shell left sitting at its prompt.

The maintenance tick now closes a shell that has sat at its prompt, with
nothing written to its recording, for `[shells] idle_reap_secs`.

## Behaviour

1. **Config.** Add `[shells] idle_reap_secs` (`u64`, default `3600`) to
   `ShellsConfig` (`src/config/shells.rs`), next to `detach_close_secs`.
   `0` means **never reap**. Document it wherever `detach_close_secs` is
   documented: `assets/etc/config.toml`, `README.md` and
   `docs/architecture.md`.
2. **`reap_idle_shells(reg, idle_secs, now_unix) -> Vec<String>`** is a
   new `pub(crate)` async fn in
   `src/daemon/executor/knowledge/shell_life.rs`, beside
   `close_after_detach`. It returns the ids it closed. A shell is closed
   only when **all** of the following hold:
   - `idle_secs > 0`;
   - its owner is not `Owner::User` (the user's `/shell` shells are never
     reaped);
   - it has no `in_flight` command;
   - `exited_unix` is `None` (a dead host is `reap_exited`'s job);
   - it is not an `Owner::Chat(s)` shell whose session is present
     (`presence_count(s) > 0`);
   - its recording is silent: the modification time of its cast
     (`launch::resolve_cast(id, started_unix)`) is `idle_secs` or more
     before `now_unix`. A missing or unreadable cast **skips** the shell,
     because its silence cannot be measured;
   - a live `probe_foreground` reads `Foreground::Prompt`. `Program`,
     `Paused`, `Unknown` and `Exited` all skip the shell. A silent
     long-running command is never killed.
3. **The close** goes through `close_shell_with`, with the record's own
   owner as the caller. It logs one `info` line naming the closed ids,
   only when there are any.
4. **Wired into the tick.** The hourly block of the maintenance loop
   (`src/daemon/startup.rs`, beside `reap_exited`) calls it with the
   `startup_config` value and the same `now`.

## Constraints

- **Never hold the registry lock across an await.** Copy the candidate
  facts out under `with_shells`, then probe and close with the lock
  released. `probe_foreground` already follows this rule.
- **One way to hang up a shell.** Use `close_shell_with`. Do not add a
  second close path.
- **Tests do not touch the real `~/.daemoneye`.** Every test holds
  `crate::test_home()`. Use the in-process hosts
  `shell_lifecycle_tests.rs` already uses (`spawn_in_process_host` /
  `spawn_with_cast`), and inject `now_unix` rather than sleeping out an
  idle limit.
  - **Gotcha:** in 17b, `test_home_guard()` (the lock without the `HOME`
    redirect) let a test write into the real shells dir. It must be
    `crate::test_home()`.
- Presence in tests: register through `presence_register`, as the 17a
  tests in `shell_lifecycle_tests.rs` do, with a session id unique to the
  test, because the presence map is process-global.
- Do not change `close_after_detach`, `reap_exited` or `Owner::may_drive`.

## Spec

1. **Red tests first**, in `shell_lifecycle_tests.rs`. Run them and paste
   the real output into an Update Log entry `(red runs)`. A test naming the
   new function or field is red as a compile error.
   - `reap_idle_closes_a_silent_adopted_shell_at_its_prompt`
   - `reap_idle_skips_a_user_shell`
   - `reap_idle_skips_a_shell_with_a_command_in_flight`
   - `reap_idle_skips_a_present_chat_sessions_shell`
   - `reap_idle_closes_an_absent_chat_sessions_shell`
   - `reap_idle_skips_a_recently_written_shell` (`now_unix` within the
     limit of the cast's mtime)
   - `reap_idle_skips_a_shell_not_at_its_prompt` (a foreground program
     such as `sleep 30`)
   - `reap_idle_does_nothing_when_idle_secs_is_zero`
   - in `src/config/tests.rs`:
     `idle_reap_secs_defaults_to_3600_and_zero_is_allowed`
2. **Implement** Behaviour 1–4, running `cargo build` after each step.
3. **Mutation check:** remove the `Foreground::Prompt` requirement, run
   `cargo test --lib reap_idle_skips_a_shell_not_at_its_prompt`, and confirm
   that it fails. Restore the requirement and confirm that it passes.
   **Paste both real results** into an Update Log entry `(mutation)`. The
   summary sentence alone is not evidence; 17b's review recorded that gap.
4. **Findings:** add a `### Findings — <date>` entry of two or three lines,
   or `none`.
5. **End-to-end:** run § End-to-end verification once, and paste its
   output.
6. **Commit** as
   `fix: the maintenance tick closes shells idle at their prompt past [shells] idle_reap_secs (M36 phase-17c)`.

## Acceptance criteria

- [ ] All nine named tests pass, each printed by the end-to-end loop with
      `failed=0` and `passed=1` or more.
- [ ] `reap_fn=1` or more: `shell_life.rs` defines
      `fn reap_idle_shells`. Today it is 0.
- [ ] `tick_call=1` or more: `src/daemon/startup.rs` calls
      `reap_idle_shells`. Today it is 0.
- [ ] `docs=3`: `idle_reap_secs` appears in each of
      `assets/etc/config.toml`, `README.md` and `docs/architecture.md`.
      Today it is 0.
- [ ] Existing tests pass unchanged.
- [ ] `real_owner_files=0` after the full test run.
- [ ] **bug-17c-1:** `flaky_runs=0`. The loop below runs `cargo test --lib
      reap_idle` 20 times and counts the runs that do not report
      `9 passed; 0 failed`. At review it was 4 of 12.
- [ ] **bug-17c-1:** the `(mutation, bug-17c-1)` Update Log entry pastes real
      failing output for the `Owner::User` and `in_flight` filter removals,
      and the passing output after the restore.
- [ ] Hygiene, `doc_truth` and `bug_tracker` are ok, `build_warnings=0`,
      clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in reap_idle_closes_a_silent_adopted_shell_at_its_prompt reap_idle_skips_a_user_shell reap_idle_skips_a_shell_with_a_command_in_flight reap_idle_skips_a_present_chat_sessions_shell reap_idle_closes_an_absent_chat_sessions_shell reap_idle_skips_a_recently_written_shell reap_idle_skips_a_shell_not_at_its_prompt reap_idle_does_nothing_when_idle_secs_is_zero idle_reap_secs_defaults_to_3600_and_zero_is_allowed; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c 'fn reap_idle_shells' src/daemon/executor/knowledge/shell_life.rs | sed 's/^/reap_fn=/'
grep -c 'reap_idle_shells' src/daemon/startup.rs | sed 's/^/tick_call=/'
n=0; for f in assets/etc/config.toml README.md docs/architecture.md; do grep -q 'idle_reap_secs' "$f" && n=$((n+1)); done; echo "docs=$n"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
n=0; for i in $(seq 1 20); do cargo test --lib reap_idle 2>&1 | grep -q '9 passed; 0 failed' || n=$((n+1)); done; echo "flaky_runs=$n"
ls ~/.daemoneye/var/run/shells/*.owner 2>/dev/null | wc -l | sed 's/^/real_owner_files=/'
E2E
```

## Authorizations

- **Edit:**
  - `src/daemon/executor/knowledge/shell_life.rs` and
    `shell_lifecycle_tests.rs`;
  - `src/config/shells.rs` and `src/config/tests.rs`;
  - the hourly block of `src/daemon/startup.rs`;
  - the three docs named in Behaviour 1.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, add or raise its entry and give the reason in Findings. Do not split
  code to fit.

## Out of scope

- Removing a closed shell's `<id>.owner` / `<id>.sandbox` sidecars on close
  (sweep_stale does it).
- Handing an `Owner::Adopted` shell to a session.
- A reap cadence other than the existing hourly block.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-08 18:47 (started)

Executor: Claude (Opus 4.6, headless run). Status flipped `todo →
in-progress` and the milestone README's 17c row updated to match. Beginning
Task 1 (red tests first) against `shell_lifecycle_tests.rs` and
`src/config/tests.rs`.

### Update — 2026-11-08 (escalation)

**Chosen lever:** resume
**Rationale:** the run ran out at 512 turns, most of them spent finding where
the in-process test hosts write their casts. The partial tree is close to
done: config, `reap_idle_shells` and all nine named tests pass. What remains
is the tick wiring, the three docs, two small fixes and the closing steps.
The spec was not the problem.

### Update — 2026-11-08 19:20 (resumed, completed work)

Executor: Claude (Opus 4.6, headless resume run). Built on the resumed tree.
Both review fixes applied to `reap_idle_shells`: the presence filter moved out
of the `with_shells` closure (the presence lock is no longer taken while the
registry lock is held), and the candidate tuple now carries the record's own
`ShellId`, so no `ShellId::parse` fallback probing s0 exists. Wired into the
hourly tick beside `reap_exited` in `src/daemon/startup.rs` (exported as
`pub(crate) use shell::reap_idle_shells` from `knowledge/mod.rs` since the
module is private). Zero build warnings after wiring. Documented
`idle_reap_secs` in `assets/etc/config.toml`, `README.md` and
`docs/architecture.md`. Hygiene allowlist entry for `run_daemon` raised
598 → 608 for the six-line tick call.

### Update — 2026-11-08 (mutation)

The `Foreground::Prompt` requirement was removed (probe result discarded):

```
test daemon::executor::knowledge::shell::tests::lifecycle::reap_idle_skips_a_shell_not_at_its_prompt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2711 filtered out; finished in 0.11s
```

Restored, the requirement is live again:

```
test daemon::executor::knowledge::shell::tests::lifecycle::reap_idle_skips_a_shell_not_at_its_prompt ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2711 filtered out; finished in 0.06s
```

### Findings — 2026-11-08

- `tests/hygiene_allowlist.txt` does not admit trailing `# comment`
  annotations (they parse as malformed), so the `run_daemon` raise is a bare
  count line; the reason is recorded in the Update Log above instead.
- Dead helper `backdate_idle_cast` (left by the resumed tree; the tests
  freshen mtime inline) removed rather than allowlisted.

### Update — 2026-11-08 (end-to-end verification)

Full § End-to-end verification transcript (`/tmp/e2e-17c.txt`):

```
reap_idle_closes_a_silent_adopted_shell_at_its_prompt passed=1 failed=0
reap_idle_skips_a_user_shell passed=1 failed=0
reap_idle_skips_a_shell_with_a_command_in_flight passed=1 failed=0
reap_idle_skips_a_present_chat_sessions_shell passed=1 failed=0
reap_idle_closes_an_absent_chat_sessions_shell passed=1 failed=0
reap_idle_skips_a_recently_written_shell passed=1 failed=0
reap_idle_skips_a_shell_not_at_its_prompt passed=1 failed=0
reap_idle_does_nothing_when_idle_secs_is_zero passed=1 failed=0
idle_reap_secs_defaults_to_3600_and_zero_is_allowed passed=1 failed=0
reap_fn=1
tick_call=1
docs=3
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.92s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s
fmt_exit=0
passed=2791 failed=0
real_owner_files=0
```

### Update — 2026-11-08 19:34 (complete, server-authored)

**Summary:** </think>Done. Phase doc status remains `in-progress` (server flips to review). Summarize.</think>
**Summary**

Phase 17c (idle-reap) is complete on commit `b965277`. ## Notes for review

- **What was built:** the resumed run applied the architect's two fixes to `reap_idle_shells` — the presence filter now runs *after* the `with_shells` closure (no presence-lock-under-registry-lock nesting), and candidate tuples carry the record's own `ShellId` so the `ShellId::parse(…).unwrap_or(ShellId::from_index(0))` s0 fallback is gone — then wired the call into the hourly maintenance block beside `reap_exited` in `src/daemon/startup.rs` (`crate::daemon::executor::knowledge::reap_idle_shells`, re-exported `pub(crate)` from `knowledge/mod.rs` because the `shell` submodule is private), and documented `idle_reap_secs` beside `detach_close_secs` in all three docs.
- **Mutation check**: with the `Foreground::Prompt` requirement removed, `reap_idle_skips_a_shell_not_at_its_prompt` FAILED (`0 passed; 1 failed`); restored, it PASSES. Both real `test result:` lines are pasted in the `(mutation)` Update Log entry.
- **E2E verification** (full transcript in the end-to-end Update Log entry, captured mechanically to `/tmp/e2e-17c.txt`): all nine named tests `passed=1 failed=0`, `reap_fn=1`, `tick_call=1`, `docs=3`, `build_warnings=0`, hygiene/doc_truth/bug_tracker all ok, clippy clean, `fmt_exit=0`, full suite `passed=2791 failed=0`, `real_owner_files=0`.
- **Two deviations to know about:** (1) the hygiene allowlist parser rejects trailing `# comment` annotations (they registered as malformed, failing `allowlist_has_no_stale_entries`), so the authorized `run_daemon` raise (598 → 608 for the six-line tick call) is a bare count line with the reason recorded in a Findings entry instead; (2) the resumed tree's unused `backdate_idle_cast` test helper (dead code under clippy `-D warnings`) was deleted rather than allowlisted — the tests set/freshen cast mtimes inline. Also fixed en route: the first wiring attempt used the private `knowledge::shell::` path (verifier compile error), resolved via the `pub(crate)` re-export.
- Findings/`none`-beyond-the-two items above; no TODOs, no unsafe, no new dependencies. Status left at `in-progress` per the lifecycle split; working tree clean.

**Executor:** glm-5.3-flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
. ok
test functions_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.91s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g5_depth_limit_enforced ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test ipc_ask_round_trip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


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
     Running unittests src/lib.rs (target/debug/deps/daemoneye-2914766807b85c19)
kill: (91188): No such process
kill: (-91866): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-cdde14477076ad75)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-345455f6ee91564c)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-476c3973573a21f0)
     Running tests/hygiene.rs (target/debug/deps/hygiene-98695f326ced6daf)
     Running tests/integration.rs (target/debug/deps/integration-f0e5ce1af26d5698)
     Running tests/isolation.rs (target/debug/deps/isolation-80b73b54a6043572)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-45311796544630a3)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-b06e25869468a096)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +2 -0
- `assets/etc/config.toml` — +6 -0
- `docs/architecture.md` — +6 -1
- `docs/dev/milestones/M36-ux-and-audit/phase-17c-idle-reap.md` — +82 -1
- `src/daemon/executor/knowledge/mod.rs` — +1 -0
- `src/daemon/executor/knowledge/shell_life.rs` — +23 -22
- `src/daemon/executor/knowledge/shell_lifecycle_tests.rs` — +391 -0
- `src/daemon/startup.rs` — +6 -0
- `tests/hygiene_allowlist.txt` — +1 -1

**Commit:** b965277091d35dbd349f71217d08d7ff131a6985

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-11-08 (bounce, bug-17c-1)

Status `review` → `in-progress`. See
[`bugs/bug-17c-1.md`](bugs/bug-17c-1.md). The two "closes" tests derive
their idle window from spawn timing and fail about 1 run in 3. The two
owner/in-flight skip tests don't establish the other conditions first.

**Notes for executor:** green gates and a clean tree are **expected** here
and are **not** evidence the phase is done. Everything else in this phase is
approved: the production code, the tick wiring, the docs and the
`run_daemon` allowlist raise. Do not touch them. The only work left is in
the idle-reap section of `shell_lifecycle_tests.rs`:
1. Make the idle precondition explicit in each test, for example by
   backdating the cast mtime.
2. Give the skip tests a `Prompt` precondition.
3. Run the two pasted mutations.
4. Re-run the E2E. It must print `flaky_runs=0`, and the test count stays
   at **2791**.
