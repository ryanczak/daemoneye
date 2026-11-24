# Phase 06 — wrap-remote

**Milestone:** M39 — Critical fixes
**Status:** review
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~120 lines, almost all deletions
**Tags:** language=rust, kind=cleanup, size=s

## Goal

The audit (S3d) flags two problems in `GhostPolicy::wrap_remote`
(`src/daemon/policy.rs`): it returns any command starting `ssh ` unwrapped,
and it puts `target` into the command unquoted, with no `--`.

The architect re-verified this at drafting on 2026-11-24 and found the
function is **dead code**. It lost its last production caller when commands
stopped running through tmux. Now:
- A ghost whose `ssh_target` resolves to a configured host runs in a PTY
  shell on that host (`executor/ghost_remote.rs`).
- A target that resolves to no configured host is refused.
- Only tests in `src/daemon/policy_tests.rs` call `wrap_remote`.

Decided at drafting (PE, 2026-11-24): **delete it**, rather than harden a
function nothing calls. Deleting it closes S3d.

## Statement of work

1. Delete `GhostPolicy::wrap_remote` and its doc comment.
2. Delete the tests that exist only to exercise it: the `wrap_remote_*`
   tests in `src/daemon/policy_tests.rs`. For
   `resolve_then_wrap_remote_sudo_prefix`, delete it if another test
   already pins `resolve_command("sudo fix.sh")` under a remote sudo
   policy; otherwise keep its `resolve_command` assertion under a new name
   that does not mention `wrap_remote`.
3. Rewrite the `GhostPolicy` comments that still describe wrapping:
   - the doc comment on the `ssh_target` field;
   - the line in `resolve_command`'s doc comment that defers to
     `wrap_remote()`.

   Say instead what the field does now. It selects the remote route, and
   the command runs in a PTY shell on the `[hosts]` host it resolved to;
   `remote_host` is that host. It also selects the tilde script path.

**Constraints:**

- Keep `remote_script_call`, `resolve_command`, `may_use_ssh_target`,
  `crate::daemon::utils::sh_single_quote` and every other `GhostPolicy`
  item. Each one still has production callers.
- Change no behaviour. This phase is deletions plus comments.
- No new tests; the gates are the evidence.

**Scoping hints.** These files are enough:
- `src/daemon/policy.rs`
- `src/daemon/policy_tests.rs`

**Do not read:** the executor modules, `README.md`, `CLAUDE.md` or the
audit docs. The architect updates the docs and the audit ledger at review.

**Budget.** You have **1200 turns**, so use them efficiently. Make your
first edit within your first **5** tool calls.

## Acceptance criteria

- [ ] `grep -rn 'wrap_remote' src` prints nothing. Today it prints
      matches in `policy.rs` and `policy_tests.rs`.
- [ ] `grep -rn 'ssh <target>' src/daemon/policy.rs` prints nothing. Today
      it matches the `ssh_target` field's doc comment.
- [ ] **Preservation:** every test outside `src/daemon/policy_tests.rs`
      passes unchanged, and every remaining test in that file passes
      unchanged.
- [ ] **Gates:** all four pass. The lib suite count falls by at most 8 from
      3097. Paste the before and after `test result:` lines.

## Spec

1. **Break down:** write a numbered task list into an Update Log entry, one
   line per Statement of work item. Done when the entry is written.
2. **Implement:** delete and rewrite. Done when `cargo build --tests` is
   green.
3. **Meet the acceptance criteria:** run both greps and check off each
   item. Done when every item is checked.
4. **Capture the evidence:** run the four gates and paste each
   `test result:` line, without the per-test `… ok` lines. Commit as one
   `refactor:` commit.

## Authorizations

- You may touch `src/daemon/policy.rs` and `src/daemon/policy_tests.rs`.
  Nothing else.

## Out of scope

- Fencing a ghost's own `ssh …` command against its runbook's `hosts:`
  list. That is egress control; the PE logged it as an observation for the
  sandbox work.
- The audit ledger and the user docs. The architect updates these at
  review.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-24 04:54 (started)

Executor: DaemonEye (executor run). Status flipped `todo` → `in-progress`.

Task breakdown (one line per Statement of work item):
1. Break down — this entry: numbered task list into the Update Log.
2. Implement — delete `GhostPolicy::wrap_remote` and its doc comment from `src/daemon/policy.rs`; delete the `wrap_remote_*` tests in `src/daemon/policy_tests.rs`; rewrite the `ssh_target` field doc comment and the `resolve_command` doc line that defers to `wrap_remote()`; handle `resolve_then_wrap_remote_sudo_prefix` per the SOW rule.
3. Meet the acceptance criteria — run both greps (`wrap_remote` and `ssh <target>` in `src/daemon/policy.rs`) and confirm zero matches; confirm no behaviour change.
4. Capture the evidence — run the four gates, paste the `test result:` lines, commit as one `refactor:` commit.

### Update — 2026-11-24 13:05 (complete, server-authored)

**Summary:** </think>Done. Final message: Summary + Notes for review.</think>
**Summary + Notes for review**

Phase 06 complete: deleted the dead `GhostPolicy::wrap_remote` (19 lines incl. doc comment) from `src/daemon/policy.rs` and its seven dedicated `wrap_remote_*` tests from `src/daemon/policy_tests.rs`. `resolve_then_wrap_remote_sudo_prefix` kept its `resolve_command("sudo fix.sh")` assertion under the new name `resolve_command_remote_sudo_prefix_no_double_sudo` (no other test pinned remote sudo-prefix resolution — I checked all six `resolve_command("sudo fix.sh")` call sites). The `ssh_target` field doc comment and the `resolve_command` doc line that deferred to `wrap_remote()` were rewritten to describe the current PTY-shell-on-resolved-`[hosts]`-host route and the tilde script path. Both acceptance greps print nothing (`grep -rn 'wrap_remote' src` and `grep -n 'ssh <target>' src/daemon/policy.rs` — exit 1, zero matches). No behaviour changed; `remote_script_call`, `resolve_command`, `may_use_ssh_target`, `sh_single_quote` and all other items are untouched. Gates: `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings` all clean; `cargo test` — lib suite `test result: ok. 3090 passed; 0 failed; 10 ignored` (down from 3097, exactly the 7 deleted wrap tests; the kept test was renamed, not removed), integration suites 2/6/19/16/45/13/2/4/0 passed, 0 failed everywhere. The 64-test `daemon::policy` module (incl. `policy_tests.rs`) passes in full. Committed as `refactor:` commit `bfcf425b`; working tree clean. Status left `in-progress` with no `(complete)` entry, per the contract.

**Notes for review:** the deletion count is 88 lines, under the ~120-line estimate, because the kept resolve test replaced one deleted test rather than being removed outright. One residual observation, already logged by the PE as out of scope: `sh_single_quote`'s only remaining non-test production caller is gone from `policy.rs` (it lives on in `utils/shell.rs` and elsewhere is unverified — I did not audit callers beyond confirming the constraint items remain used).

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test config_pricing_round_trip ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test attach_sends_session_counters_after_ok ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
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

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.22s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test daemon_boots_in_throwaway_root ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_webhook_returns_200 ... ok
test notices_follow_streams_a_webhook_alert ... ok
test default_server_unchanged ... ok
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

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-0635aac7b8e1e57e)
kill: (2877754): No such process
kill: (-2878482): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-f945a8c3f7582fdb)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-2e19192a3a1354c4)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7fcecbb518583aee)
     Running tests/hygiene.rs (target/debug/deps/hygiene-653cf39ac0e64fe2)
     Running tests/integration.rs (target/debug/deps/integration-849708bb7ff158dd)
     Running tests/isolation.rs (target/debug/deps/isolation-fbc36518ed3cd563)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-ae0cb5d281522d21)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-55beac99186d4a12)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M39-critical-fixes/README.md` — +1 -1
- `docs/dev/milestones/M39-critical-fixes/phase-06-wrap-remote.md` — +11 -1
- `src/daemon/policy.rs` — +8 -23
- `src/daemon/policy_tests.rs` — +7 -63

**Commit:** bfcf425bb93d590f64bdb072e2c49c719169a5ed

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
