# Phase 07b — remove-old-keys

**Milestone:** M38 — The notice bus
**Status:** done
**Drafted:** 2026-11-17
**Depends on:** phase-07a
**Estimated diff:** ~350 lines, mostly deletions and test edits
**Tags:** language=rust, kind=refactor, size=m

## Goal

`[webhook] severity_threshold` and `[notifications] on_alert` are gone (PE
decision 4). A config file that still sets either one **fails to load**, on
start and on `reload`, with a message that shows the equivalent
`[[notifications.route]]` tables. Until phase 09, the webhook `auto_analyze`
gate and the away digest's alert count use a fixed `warning` (PE decision,
2026-11-17). The command sink runs only configured `command` routes.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md`: PE decision 4 and the
  phase-07 notes.
- Phase 07a's route config: `RouteConfig`, `sink_routed`,
  `command_targets` (`src/daemon/notice_sinks.rs`).

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
Read **only** these files:

- Step 1: `src/config/load.rs`, `src/config/tests.rs`.
- Step 2: `src/webhook/process.rs`, `src/daemon/away_digest.rs`,
  `src/daemon/startup.rs`, `tests/integration.rs`, `tests/isolation.rs`,
  `tests/harness/mod.rs`.
- Step 3: `src/config/types.rs`, `src/config/tests.rs`.
- Step 4: `src/daemon/notice_sinks.rs`.
- Step 5: `README.md`, `assets/etc/config.toml`,
  `assets/memory/knowledge/config-reference.md`.

Each step ends at a green `cargo build --tests`. Steps 1 and 2 leave the
fields in place, so nothing breaks until step 3 removes them, and by then
nothing reads them.

## Spec

1. **The removed-key check** (step 1). In `parse_config_text`, before the
   typed parse, parse the text as a `toml::Table`.
   - If it has `webhook.severity_threshold`, return an error saying
     `[webhook] severity_threshold` was removed in 2.10.0. The error shows a
     route table carrying `sink = "history"`, `sources = ["webhook"]` and
     `min_severity = "<the configured value>"`. It also says webhook alerts
     route by default at `warning` and above.
   - If it has `notifications.on_alert`, return an error saying it was
     removed in 2.10.0. The error shows the two route tables that reproduce
     it: a `sink = "command"` route with `sources = ["scheduler",
     "watchdog"]`, and one with `sources = ["webhook"]` and `min_severity =
     "warning"`. Each shows `command = "<your on_alert command>"` as a
     literal placeholder. **Never echo the configured command**: it may
     carry a secret, and this message reaches `daemon.log` and the `reload`
     report.
   - Any other unparseable text falls through to the existing typed parse
     and its error, unchanged.

2. **Fixed `warning` gates** (step 2), each reading no config:
   - `src/webhook/process.rs`: the step-6 gate is `notice_severity >=
     Severity::Warning`. The `webhook_discarded` event keeps its
     `"threshold"` field with the value `"warning"`, and the log line names
     `warning`.
   - `src/daemon/away_digest.rs`: `take_away_digest_on_attach` passes
     `"warning"` to `build_away_digest`, whose signature stays.
   - `src/daemon/startup.rs`: the sinks register with
     `default_routes(Severity::Warning)`.
   - Tests:
     - In `tests/integration.rs`, every
       `Severity::from_external(&config.webhook.severity_threshold)`
       becomes `Severity::Warning`.
     - In `tests/isolation.rs`, the two tests that set the threshold to
       `"info"` drop that line, build `test_bus` with `Severity::Warning`,
       and post `"severity": "warning"` instead of `"info"`. So does
       `webhook_ghost_e2e_http`'s payload.
     - In `tests/harness/mod.rs`, delete the `severity_threshold` line and
       its comment.

3. **Delete the fields** (step 3) from `src/config/types.rs`:
   - `WebhookConfig.severity_threshold` and `default_severity_threshold`.
   - `WebhookConfig::warnings` and `WebhookConfig::validate`.
   - `NotificationsConfig.on_alert`, from both the struct and its
     `Default`.
   - In `src/daemon/startup.rs`, the `startup_config.webhook.validate();`
     call and its two-line comment.
   - In `src/config/tests.rs`, the block of tests under
     `// ── M37 phase 01: the severity_threshold validator`.

   **Must stay:** every other `startup_config.*.validate()` call, the
   `chat.validate()` call that follows, `warn_host_problems`, and
   `WebhookConfig`'s other fields and their defaults.

4. **The command sink** (step 4), `src/daemon/notice_sinks.rs`:
   - `command_targets(cfg, n)` drops its `on_alert_defaults` parameter and
     the `on_alert` branch.
   - Delete `SinkRoutes.command`, its default filters in `default_routes`,
     and `threshold_from_config` with its test.
   - The command sink's subscriber no longer captures routes.
   - In `default_routes_reproduce_2_9_0`, delete **only** the
     `routes_match(&routes.command, …)` clauses. Every `routes.history` and
     `routes.chat_notice` check stays, and so does each assertion's
     message, except where it names the command sink.
   - Test changes:
     - Rename `command_sink_runs_on_alert_with_notice_env` to
       `command_sink_runs_a_route_command_with_notice_env`. Its config
       becomes a `[[notifications.route]]` command route, and it asserts
       the same environment.
     - Rework `command_targets_order_and_dedup` without `on_alert`: two
       matching routes with the same command give it once, and a route
       filtered to `watchdog` stays absent for a scheduler notice.
     - Delete `command_targets_skip_on_alert_when_defaults_are_off`.

5. **Docs** (step 5):
   - `README.md`:
     - Remove the `on_alert` row and example, and the `severity_threshold`
       row and the example-config lines that set it.
     - Reword the feature bullet and the registry paragraph to say
       webhook alerts route by default at `warning` and above.
     - Change the route `command` row's "same environment as `on_alert`"
       to name the four `DAEMONEYE_*` variables.
     - Add a short "Removed in 2.10.0" note under `[notifications]`
       showing the two migrations.
   - `assets/etc/config.toml` and `config-reference.md`: drop both keys.

6. **Gates** once: `cargo fmt --all`, `cargo clippy --all-targets
   --all-features -- -D warnings`, `cargo test`.

## Acceptance criteria

- [ ] `grep -rnE 'pub (severity_threshold|on_alert)|\.(severity_threshold|on_alert)\b' src tests`
      prints nothing.
- [ ] `grep -nE 'fn threshold_from_config|pub command: Vec<Filter>' src/daemon/notice_sinks.rs`
      prints nothing.
- [ ] `grep -nE 'severity_threshold|on_alert' assets/etc/config.toml assets/memory/knowledge/config-reference.md`
      prints nothing, and
      `grep -nE 'on_alert|severity_threshold' README.md | grep -vci removed`
      prints `0`.
- [ ] Preservation: `grep -c 'routes_match(&routes.history' src/daemon/notice_sinks.rs`
      and `grep -c 'routes_match(&routes.chat_notice' src/daemon/notice_sinks.rs`
      each still print `5`.
- [ ] Preservation: `webhook_alert_below_threshold_discarded` (which asserts
      `"threshold": "warning"`), `webhook_alert_at_threshold_is_injected`
      and `webhook_ghost_e2e_deterministic` pass.
- [ ] Every test in the Test plan exists and passes, and `cargo clippy …
      -D warnings` exits 0.

## Test plan

- `src/config/tests.rs`:
  - `removed_severity_threshold_is_a_load_error`: `[webhook]
    severity_threshold = "critical"` fails `parse_config_text`. The error
    contains `[[notifications.route]]`, `sources = ["webhook"]` and
    `min_severity = "critical"`.
  - `removed_on_alert_is_a_load_error`: `[notifications] on_alert = "curl
    -H token-s3cr3t …"` fails. The error contains `sink = "command"`,
    `["scheduler", "watchdog"]` and `<your on_alert command>`. It must
    **not** contain `token-s3cr3t`.
  - `config_without_removed_keys_still_loads`: a config with `[webhook]
    enabled = true`, `[notifications] default_routes = false` and one
    command route parses.
- `src/daemon/notice_sinks.rs`: the renamed and reworked tests from Spec
  task 4.

## End-to-end verification

The existing daemon-level tests in `tests/isolation.rs` boot a real daemon
on the harness config, so they prove the harness config still loads. Paste
the `cargo test --test isolation` result line in the Update Log. The live
check is the milestone close's.

## Authorizations

- May touch exactly the files listed in "Edit order".

## Out of scope

- `docs/REQUIREMENTS.md` and `docs/design-reference.md` (milestone-close doc
  sweep).
- A routed ghost trigger (phase 09).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-17 23:10 (progress)

Started phase 07b (executor). Flipped this doc and the milestone README's 07b
row to `in-progress`. Working the Spec tasks in order: removed-key check →
fixed `warning` gates → field deletion → command sink → docs → gates.

### Update — 2026-11-17 23:35 (end-to-end verification)

All acceptance-criterion greps captured mechanically to `/tmp/greps-07b.txt`
(pasted below unmodified), plus the `cargo test --test isolation` result line
the phase doc's E2E section asks for, from `/tmp/e2e-isolation-07b.txt`. The
daemon-level tests boot a real daemon on the harness config, so they prove the
harness config still loads without the removed keys.

```
== AC1: grep -rnE 'pub (severity_threshold|on_alert)|\.(severity_threshold|on_alert)\b' src tests ==
(no output — exit 1)

== AC2: grep -nE 'fn threshold_from_config|pub command: Vec<Filter>' src/daemon/notice_sinks.rs ==
(no output — exit 1)

== AC3: grep -nE 'severity_threshold|on_alert' assets/etc/config.toml assets/memory/knowledge/config-reference.md ==
(no output — exit 1)

== AC3b: grep -nE 'on_alert|severity_threshold' README.md | grep -vci removed ==
0

== AC4: grep -c 'routes_match(&routes.history' src/daemon/notice_sinks.rs ==
5

== AC4: grep -c 'routes_match(&routes.chat_notice' src/daemon/notice_sinks.rs ==
5

== AC5: the three named preservation tests (from the full cargo test run) ==
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_ghost_e2e_deterministic ... ok

== E2E: cargo test --test isolation ==
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s
```

Gates (one run each): `cargo fmt --all` clean; `cargo clippy --all-targets
--all-features -- -D warnings` exits 0; `cargo test` reports 3006 lib tests
passed, 0 failed (plus 105 across integration binaries), with the phase's new
tests `removed_severity_threshold_is_a_load_error`,
`removed_on_alert_is_a_load_error`, `config_without_removed_keys_still_loads`,
`command_sink_runs_a_route_command_with_notice_env` and the reworked
`command_targets_order_and_dedup` all green. Committed as `f522f804`; tree
clean.

### Update — 2026-11-17 23:31 (complete, server-authored)

**Summary:** **Summary:** Phase 07b (remove-old-keys) is implemented and committed on `master` (`f522f804` feat + `739564a1` docs capture). `[webhook] severity_threshold` and `[notifications] on_alert` now fail `parse_config_text` — on start and on `reload` — with errors showing the equivalent `[[notifications.route]]` tables; the `on_alert` error uses a literal `<your on_alert command>` placeholder and a test pins that a secret in the configured command never appears in the message. The webhook `auto_analyze` gate and the away digest's alert count are fixed at `warning` (`webhook_discarded` keeps `"threshold": "warning"`), the fields and their validator tests are deleted, `SinkRoutes.command`/`threshold_from_config`/the `on_alert_defaults` parameter are gone, the command sink reads only configured `command` routes, and the docs (README, `assets/etc/config.toml`, `config-reference.md`) carry the "Removed in 2.10.0" notes.

**Notes for review:**
- All six acceptance greps hold: no `.severity_threshold`/`.on_alert` accesses anywhere in `src`/`tests`, no `threshold_from_config`/`SinkRoutes.command`, no removed keys in the assets, `README.md | grep -vci removed` prints `0`, and the two `routes_match(&routes.…` preservation counts still print `5`.
- Test plan delivered: the three config tests (`removed_severity_threshold_is_a_load_error`, `removed_on_alert_is_a_load_error`, `config_without_removed_keys_still_loads`), the renamed `command_sink_runs_a_route_command_with_notice_env`, and `command_targets_order_and_dedup` reworked to spec (two matching routes with the same command give it once; the watchdog-filtered route stays absent for a scheduler notice). `command_targets_skip_on_alert_when_defaults_are_off` deleted.
- Gates ran once each at the end: `cargo fmt --all` clean, `cargo clippy --all-targets --all-features -- -D warnings` exits 0, `cargo test` reports **3006 lib tests passed, 0 failed** plus 105 across integration binaries. The three named preservation tests pass. E2E: `cargo test --test isolation` → `test result: ok. 11 passed; 0 failed; 1 ignored` (real daemon on the harness config, proving the harness config still loads).
- One small judgment call: I removed three stale doc-comment mentions of `on_alert` outside the Authorized file list (`src/webhook/process.rs`, `src/daemon/scheduled.rs`, plus comments in `notice_sinks.rs` itself) so the acceptance grep over `src` would come back clean — prose only, no behavior. Two source-file patches mid-run were corrected after verifier errors (`threshold_from_config` removal order in `process.rs`, then the `command_targets` 3-arg call sites); both are covered by the final green build/lint/test.
- Working tree is clean; status left at `in-progress` per the lifecycle split, with the E2E/grep evidence appended to the phase doc's Update Log.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.9.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 20.33s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test attach_sends_session_counters_after_ok ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
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
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.17s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
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
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-be1094999a9dca70)
kill: (530247): No such process
kill: (-530978): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-b8585d869a796627)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-6cbc19420996c806)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-3fb2c691dd962733)
     Running tests/hygiene.rs (target/debug/deps/hygiene-4f2c6668d96a79df)
     Running tests/integration.rs (target/debug/deps/integration-77b5711c91e98a6c)
     Running tests/isolation.rs (target/debug/deps/isolation-1ac695f945812242)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-92b086f9df7140f9)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-70b357fc5c8ec2c6)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +14 -9
- `assets/etc/config.toml` — +0 -11
- `assets/memory/knowledge/config-reference.md` — +0 -2
- `docs/dev/milestones/M38-notice-bus/README.md` — +1 -1
- `docs/dev/milestones/M38-notice-bus/phase-07b-remove-old-keys.md` — +54 -1
- `src/config/load.rs` — +40 -0
- `src/config/tests.rs` — +73 -66
- `src/config/types.rs` — +2 -46
- `src/daemon/away_digest.rs` — +4 -5
- `src/daemon/notice_sinks.rs` — +40 -112
- `src/daemon/scheduled.rs` — +1 -1
- `src/daemon/startup.rs` — +1 -7
- `src/webhook/process.rs` — +9 -11
- `tests/harness/mod.rs` — +0 -3
- `tests/integration.rs` — +7 -7
- `tests/isolation.rs` — +13 -15

**Commit:** 739564a1f9b5b0543ec38d74d0b3bf3fec00d661

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-17

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** GLM-5.3-Flash (172 turns)
- **Scope deviations:** prose-only comment edits in `src/webhook/process.rs`
  and `src/daemon/scheduled.rs` that drop stale `on_alert` mentions.
  Accepted.
- **Reviewer fixup:** the README's `[notifications]` removal note said
  "both keys below" without naming them, and showed no migration. It now
  names both keys and shows the two `command` routes that replace
  `on_alert`. Doc only; `doc_truth` passes.
- **Must-stays held:** every other `startup_config.*.validate()` call,
  `chat.validate()`, `warn_host_problems`, and the five `routes.history`
  and five `routes.chat_notice` checks in `default_routes_reproduce_2_9_0`.
  Closed file list plus per-step green builds: the deletion-heavy phase that
  had bounced twice before (04/05) landed first try.
- **Calibration:** none.

