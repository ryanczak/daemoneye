# Phase 04 — local-provider-pricing

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** done
**Drafted:** 2026-11-14
**Depends on:** nothing in M37
**Estimated diff:** ~80 lines, most of it tests and docs
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Fix GitHub issue #6. A `[models.<name>]` entry with `provider = "ollama"`
or `"lmstudio"` that sets any `*_cost_per_mtok` rate is priced at those
rates, with `PricingSource::UserConfig`. Today `ModelEntry::pricing()`
returns `Pricing::zero()` for local providers before it looks at the user's
rates, so every `ai_cost` event, `daemoneye costs`, the status bar's cost
slot and the daily spend cap see `$0` for a self-hosted model the user
priced. A local entry with **no** rate set stays free (`PricingSource::Local`,
all zero), as today.

## Architecture references

- `src/config/models.rs`: `ModelEntry::pricing`, `PricingSource`,
  `Pricing::zero`.
- `src/cost.rs`: `emit_cost` (the single cost site; unchanged),
  `compute_cost`.
- `src/config/load.rs`: `validate_pricing` (skips local providers; unchanged).

## Spec

Numbered tasks in execution order. The executor's task list is seeded from
this section: mark each one `active` when you start it and `done` when its
`cargo build` is green.

1. **Precedence** — in `ModelEntry::pricing()`, user rates win over the
   local-provider zero. Any one of the four rate fields being `Some` counts
   as "user rates set" (unset fields price at 0.0, as for remote providers
   today). Only when none is set does a local provider return
   `Pricing::zero()`. A remote provider with no rates still returns `None`.
   Update the doc comments on `pricing()` and on `PricingSource::Local` so
   neither claims local providers are always zero.
2. **Tests** — in `src/config/tests.rs`, keep
   `local_provider_pricing_is_zero` passing unchanged (it is the
   no-rates case) and add the tests in § Test plan.
3. **Docs** — `assets/etc/config.toml`: the local section's heading drops
   "always $0" and says local models are free unless rates are set (a
   one-line commented example rate is fine). `README.md` § the "Local
   models always $0.00" bullet: the same change. (`src/config/seeds.rs`
   seeds the template with `include_str!`, so it needs no edit.)
4. **Capture the evidence** — run the mutation check and the
   § End-to-end block verbatim, and paste both into one Update Log entry.

## Constraints

- `emit_cost`, `compute_cost`, `validate_pricing` and the `PricingSource`
  variants and their serde names do not change. Records already on disk
  must still deserialize.
- Tests construct `ModelEntry` values directly; they read no config file
  and write no event log.

## Test plan

- `local_provider_with_user_rates_uses_them`: for both `ollama` and
  `lmstudio`, an entry with all four rates set resolves to those rates with
  `PricingSource::UserConfig`.
- `local_provider_partial_rates_price_unset_buckets_at_zero`: an `ollama`
  entry with only `output_cost_per_mtok` set resolves to that output rate,
  0.0 for the other three, `UserConfig`.
- `local_provider_user_rates_produce_nonzero_cost`: an `ollama` entry with
  input 10.0 / output 50.0 per Mtok, fed through `compute_cost` with a
  `TokenBreakdown` of 1 000 000 input and 100 000 output tokens, gives a
  total of 15.0 (within 1e-9). Put it wherever `compute_cost` is reachable
  from a test — `src/cost.rs`'s test module is fine.

## Acceptance criteria

- [x] Every test in the test plan passes, and `local_provider_pricing_is_zero`
      still passes.
- [x] `grep -c 'always \$0' assets/etc/config.toml` prints 0 (today 1).
- [x] Mutation check, pasted with its `test result:` line: move the
      local-provider early return back above the user-rate check, confirm
      `local_provider_with_user_rates_uses_them` fails, then revert.
- [x] fmt, build (0 warnings), clippy and `cargo test` are green, with
      `failed=0` and more than 2956 passed. Hygiene,
      `doc_truth` and `bug_tracker` are ok.

## Pre-flight

Baseline at drafting (2026-11-14, `1e9c920c`): `cargo test` passed=2956, failed=0.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib local_provider 2>&1 | grep -E '^test result'
grep -c 'always \$0' assets/etc/config.toml | sed 's/^/always_zero=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Edit `src/config/models.rs`, `src/config/tests.rs`, `src/cost.rs` (tests
  only), `assets/etc/config.toml` and `README.md`.

## Out of scope

- Rewriting historical `ai_cost` records (they stay `$0`; forward-only).
- Any change to how `daemoneye costs`, the status bar or the spend ledger
  consume a `CostRecord`.
- Built-in price tables for any provider.

## Update Log

<!-- entries below this line -->

### Update — 2026-11-14 19:05 (started)

**Executor:** executor (headless)
**Status:** in-progress

Fixing GitHub issue #6: `ModelEntry::pricing()` gives user `*_cost_per_mtok`
rates precedence over the local-provider zero for `ollama`/`lmstudio`.

### Update — 2026-11-14 11:24 (mutation check)

Moved the local-provider early return back above the user-rate check and
re-ran the pinned test:

```text
$ cargo test --lib local_provider_with_user_rates_uses_them 2>&1 | grep -E '^(test result|test config::tests::local_provider_with_user_rates|---- )'
test config::tests::local_provider_with_user_rates_uses_them ... FAILED
---- config::tests::local_provider_with_user_rates_uses_them stdout ----
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2868 filtered out; finished in 0.00s
```

Reverted the mutation; `pricing()` is back to user-rates-first. The test
drives the production `ModelEntry::pricing()` the ghost's cost site calls
(break the precedence and it fails), so the mutation check proves it pins
the behaviour.

### Update — 2026-11-14 11:24 (end-to-end verification)

Ran the phase's end-to-end block mechanically, output redirected to
`/tmp/p04_e2e.txt` and pasted verbatim:

```text
$ cargo test --lib local_provider 2>&1 | grep -E '^test result'
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 2864 filtered out; finished in 0.00s
$ grep -c 'always \$0' assets/etc/config.toml | sed 's/^/always_zero=/'
always_zero=0
$ cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
build_warnings=0
$ cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=2959 failed=0
```

Hygiene `--test hygiene` ok (16 passed), `doc_truth` ok (19 passed),
`bug_tracker` ok (6 passed). Baseline was passed=2956; now 2959
(three new tests, `local_provider_pricing_is_zero` untouched).

### Update — 2026-11-14 19:27 (complete, server-authored)

**Summary:** All four spec tasks are complete and the working tree is clean.

**Summary:** Fixed issue #6 in `ModelEntry::pricing()` — user `*_cost_per_mtok` rates now win over the local-provider zero for `ollama`/`lmstudio` (any one set rate makes the entry `PricingSource::UserConfig` with unset fields at 0.0; no rates keeps the free `PricingSource::Local` zero; remote without rates still returns `None`), with doc comments on `pricing()` and `PricingSource::Local` updated to stop claiming local providers are always zero. Added the three test-plan tests (`local_provider_with_user_rates_uses_them` for both providers in `src/config/tests.rs`, `local_provider_partial_rates_price_unset_buckets_at_zero`, and `local_provider_user_rates_produce_nonzero_cost` in `src/cost.rs` driving the real `compute_cost`), and updated the local section heading in `assets/etc/config.toml` (plus a commented example rate) and the README bullet. End-to-end verification pasted verbatim into the phase doc: `local_provider` suite 5 passed, `always_zero=0`, `build_warnings=0`, full run `passed=2959 failed=0` (baseline 2956 + 3 new), and hygiene/doc_truth/bug_tracker all green.

**Notes for review:** The mutation check was run as specified — moving the local-provider early return back above the user-rate check makes `local_provider_with_user_rates_uses_them` fail (`test result: FAILED. 0 passed; 1 failed`), then reverted; both the failed transcript and the e2e block are pasted in the phase doc's Update Log. One mid-run detour worth flagging: while re-checking a line I accidentally clobbered the `let has_user` line and then restored the old precedence while repairing it — a compile error caught it and a follow-up mutation run confirmed the test correctly fails under the old behaviour before I reverted to the fixed version; the committed code is the user-rates-first fix and the final e2e numbers are from the committed state. I also made a small extra docs commit changing `input_cost_per_mtok`'s doc comment, which still told local users to "set this to Some(0.0)" — contradicted by the new behaviour; flagging in case you consider that beyond the phase's file list (it is in `src/config/models.rs`, which the phase authorizes). Status remains `in-progress` in both the phase doc and milestone README; no `(complete)` entry was written.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test attach_sends_session_counters_after_ok ... ok
test alert_list_request_returns_the_snapshot ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 40 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-e4eaed9c20b7817e)
kill: (3243929): No such process
kill: (-3244597): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-84596a3046ce91b6)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-61684e0b51107e5b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c69cc98b7b8dfe7a)
     Running tests/hygiene.rs (target/debug/deps/hygiene-1cb80d9fbff8faf7)
     Running tests/integration.rs (target/debug/deps/integration-9d0880a1c76a698b)
     Running tests/isolation.rs (target/debug/deps/isolation-882adad6d3253c89)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-bad9d4ccd89b2daf)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-3f0be99859fd1842)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +1 -1
- `assets/etc/config.toml` — +2 -1
- `docs/dev/milestones/M37-alerts-status-bar/README.md` — +1 -1
- `docs/dev/milestones/M37-alerts-status-bar/phase-04-local-provider-pricing.md` — +47 -1
- `src/config/models.rs` — +12 -7
- `src/config/tests.rs` — +43 -0
- `src/cost.rs` — +24 -0

**Commit:** 534aaddf938c3a1cda3b13f5ce4ac765ae63d687

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-14

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** GLM-5.3-Flash
- **Scope deviations:** one in-scope extra: the `input_cost_per_mtok` doc comment in `src/config/models.rs` no longer tells local users to set `Some(0.0)` (an authorized file, and a correct fix). The config template's new commented example `input_cost_per_mtok = 0.0` is harmless but mislabelled (uncommented, it would switch the entry to `UserConfig` at zero); left as is.
- **Calibration:** none. Reviewer re-run: fmt clean, 0 build warnings, clippy clean, `passed=2959 failed=0` (baseline 2956 + 3).
