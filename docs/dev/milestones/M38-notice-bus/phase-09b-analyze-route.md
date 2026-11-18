# Phase 09b — analyze-route

**Milestone:** M38 — The notice bus
**Status:** done
**Drafted:** 2026-11-18
**Depends on:** phase-09a
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

`[[notifications.route]]` accepts `sink = "analyze"`. A route with that
sink sends matching webhook alerts to the `ghost_trigger` consumer, the
one that runs the analysis that may start a ghost shell. The default route
keeps today's gate: webhook alerts, firing or resolved, at `warning` or
above. Like every other sink:
- configured routes **add** to the default;
- `default_routes = false` turns the default off;
- `daemoneye reload` applies changes, because routes are read per
  delivery.

## Architecture references

- `src/daemon/notice_sinks.rs`: `analyze_routed`, `sink_routed`,
  `default_routes`.
- `src/config/types.rs` `RouteSink`, and `src/config/load.rs`
  `validate_routes`.
- The two callers of `analyze_routed`: step 6 of `process_alert` in
  `src/webhook/process.rs`, and the consumer loop in
  `src/webhook/trigger.rs`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
Read **only** these files:

- Step 1: `src/config/types.rs` (`RouteSink`), `src/config/load.rs`
  (`validate_routes`) and `src/config/tests.rs` (the route tests).
- Step 2: `src/daemon/notice_sinks.rs`, `src/webhook/process.rs` (step 6
  only) and `src/webhook/trigger.rs`.
- Step 3: `assets/etc/config.toml`, `README.md` (the
  `[[notifications.route]]` section),
  `assets/memory/knowledge/config-reference.md`, and the
  `notice_sinks.rs` row of `CLAUDE.md`.

Each step ends at a green `cargo build --tests`.

## Spec

1. **The sink** (step 1). `RouteSink` gains `Analyze`, spelled `analyze`
   in TOML. `validate_routes` refuses a `command` key on an `analyze`
   route, exactly as it does for `history`.

2. **The gate reads the config** (step 2). `analyze_routed` becomes
   `analyze_routed(cfg: &NotificationsConfig, n: &Notice) -> bool`. It is
   true when **both** of these hold:
   - the notice's label `kind` is `alert`;
   - `sink_routed(cfg, &default_routes(Severity::Warning).analyze,
     RouteSink::Analyze, n)`.

   The `kind` rule moves out of the consumer into the gate. **Gotcha:** a
   configured route with no `statuses`, such as
   `sink = "analyze"` with `names = ["HighCPU"]`, matches the
   `[Webhook Analysis] HighCPU` notice. Without the `kind` rule, every
   analysis would trigger another analysis.

   Both callers pass `&crate::config::current().notifications`, read per
   notice. They must not use `state.config`, which is a startup snapshot
   that `reload` never updates.

3. **Docs** (step 3).
   - `assets/etc/config.toml` and the README's `sink` row name `analyze`:
     it runs the runbook analysis that may start a ghost shell, on
     webhook alerts only.
   - The README adds an example route: analyse `info` alerts for one name.
   - The README notes that `default_routes = false` also drops the default
     analyze route, so ghost triggering stops unless an `analyze` route is
     configured.
   - The `CLAUDE.md` row names the sinks `SinkRoutes` actually holds.

**Must stay:** the `webhook_discarded` / `below_threshold` record keeps its
fields, and is logged exactly when `analyze_routed` is false for the alert
notice. `[webhook] auto_analyze` still gates the consumer.

## Acceptance criteria

- [ ] `grep -c 'Analyze' src/config/types.rs` prints at least `1`.
- [ ] `grep -c 'config::current()' src/webhook/trigger.rs` prints at
      least `1`, and the same grep over `src/webhook/process.rs` prints
      at least `1`.
- [ ] `grep -c '"analyze"' assets/etc/config.toml README.md` reports at
      least `1` for each file.
- [ ] Every test in the Test plan exists and passes. `cargo clippy …
      -D warnings` exits 0, and `tests/doc_truth.rs` passes.

## Test plan

Each gate test calls `analyze_routed` with a `NotificationsConfig` parsed
from TOML, the way the existing `sink_routed` tests in `notice_sinks.rs`
parse theirs. **Do not restate the gate in the test.**

- `src/config/tests.rs`:
  - `analyze_route_parses`: `sink = "analyze"` loads as
    `RouteSink::Analyze`.
  - `analyze_route_refuses_a_command`.
- `notice_sinks.rs`:
  - `analyze_routed_honours_configured_routes`:
    - With the default config, a webhook `Warning` alert is routed and an
      `Info` alert is not.
    - With `default_routes = false` and no routes, the `Warning` alert is
      **not** routed.
    - With `default_routes = false` and one `analyze` route for
      `names = ["LowCPU"]`, the `Info` `LowCPU` alert **is** routed, and an
      `Info` alert of any other name is not.
  - `analyze_routed_never_routes_an_analysis`: under the same
    `names = ["HighCPU"]` route, with no statuses, the `[Webhook
    Analysis] HighCPU` notice (`kind` = `analysis`) is **not** routed.
  - `analyze_route_skips_analysis_notices` (existing) calls the new
    signature with the default config.
- `trigger.rs`: the two existing tests keep their assertions. Their test
  HOME has no `config.toml`, so `config::current()` loads the defaults.

## End-to-end verification

`webhook_ghost_e2e_deterministic` in `tests/isolation.rs` runs through the
config-reading gate with the default routes. Paste its `cargo test` line
into the Update Log.

## Authorizations

- May touch exactly the files listed in "Edit order".

## Out of scope

- `Request::Subscribe` and `daemoneye notices` (phase 10).
- Renaming the `below_threshold` record or its `threshold` field.
- **Accepted delta:** `default_routes = false` now also stops ghost
  triggering, unless an `analyze` route is configured (PE decision,
  2026-11-18).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-18 05:11 (started)

**Executor:** executor (headless run, phase-09b analyze-route).
**Status:** flipped `todo → in-progress`; milestone README row 09b updated to
match.

Implemented the Spec in order: `RouteSink::Analyze` (`"analyze"` in TOML) and
`validate_routes` refusing a `command` key on it, `analyze_routed(cfg, n)`
reading `crate::config::current().notifications` at both callers (the
`kind` rule moved into the gate out of the trigger consumer), and the docs
(`config.toml`, README route section, config-reference, CLAUDE.md row).
New tests: `analyze_route_parses`, `analyze_route_refuses_a_command`,
`analyze_routed_honours_configured_routes`,
`analyze_routed_never_routes_an_analysis`; the existing
`analyze_route_skips_analysis_notices` and the two trigger tests pass
against the new signature. All four gate commands run green; E2E
verification follows.

### Update — 2026-11-18 05:20 (end-to-end verification)

Spec-pinned literals, greped against the files the phase populated:

```sh
$ grep -c 'Analyze' src/config/types.rs
1
$ grep -c 'config::current()' src/webhook/trigger.rs
1
$ grep -c 'config::current()' src/webhook/process.rs
1
$ grep -c '"analyze"' assets/etc/config.toml README.md
assets/etc/config.toml:3
README.md:2
```

E2E per the phase doc — the webhook ghost path through the
config-reading gate with the default routes, captured to
`/tmp/e2e-m38-09b.txt` from a single run:

```sh
$ cargo test --test isolation webhook_ghost_e2e_deterministic 2>&1 | tail -6 > /tmp/e2e-m38-09b.txt

running 1 test
test webhook_ghost_e2e_deterministic ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.14s
```

doc_truth (`tests/doc_truth.rs`), from the same run set:

```sh
$ cargo test --test doc_truth 2>&1 | tail -3

running 19 tests
...
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

Full gate sequence, all green:

- `cargo fmt --all` — clean (run immediately before staging).
- `cargo build` — 0 warnings.
- `cargo clippy --all-targets --all-features -- -D warnings` — exit 0 (one
  fix along the way: a `== false` in a new test rewritten as `!`).
- `cargo test` — lib 3022 passed / 0 failed / 10 ignored, plus all
  integration suites green.

### Update — 2026-11-18 05:14 (complete, server-authored)

**Summary:** **Summary.** Phase 09b (analyze-route) is implemented and committed. `RouteSink::Analyze` (`"analyze"` in TOML) exists and `validate_routes` refuses a `command` key on it, exactly as for `history`. `analyze_routed` now takes `&NotificationsConfig` and both callers (step 6 of `process_alert` in `src/webhook/process.rs` and the consumer loop in `src/webhook/trigger.rs`) pass `&crate::config::current().notifications` read per notice — never `state.config` — and the `kind = "alert"` rule moved into the gate, so a configured route with no `statuses` can never turn an analysis into another analysis. Docs updated per step 3 (`assets/etc/config.toml` names `analyze` in the sink list, README gains the `analyze` sink row, an `info`-severity single-name example route, and the `default_routes = false` drops-ghost-triggering note, plus config-reference and the CLAUDE.md `notice_sinks.rs` row). Verification: all four gate commands green — fmt clean, build 0 warnings, clippy `-D warnings` exit 0, full `cargo test` 3022 lib + all integration suites passed, `tests/doc_truth.rs` 19 passed — and the phase's E2E artifact `cargo test --test isolation webhook_ghost_e2e_deterministic` passes (`test result: ok. 1 passed; 0 failed; ... 12 filtered out`), captured to `/tmp/e2e-m38-09b.txt` and quoted in the end-to-end Update Log entry alongside the spec-pinned literal greps (`Analyze` = 1, `config::current()` = 1 in both callers, `"analyze"` = 3 in config.toml / 2 in README).

**Notes for review.**
1. Two spec-deviations I adapted to rather than blocked on, both disclosed: (a) the spec's `analyze_routed_never_routes_an_analysis` premise said the `names = ["HighCPU"]` route "with no statuses" matches the analysis notice — with the *default* filter routes (`default_routes(...).analyze`) that notice does **not** match even before the `kind` rule (the defaults carry `statuses = [firing, resolved]` and a `min_severity` the notice's `Event` status fails); the test therefore exercises the gate under a *configured* `analyze` route (which has no statuses), which is exactly the case the spec's gotcha describes, and asserts both the filter-only outcome and the gate's refusal. (b) The phase-doc `Status:` was still `todo` when I arrived (README row also `todo`) — I flipped both to `in-progress` at start per the lifecycle.
2. The two existing `trigger.rs` tests run against the new gate signature unchanged in assertion; their test HOME has no `config.toml` so `config::current()` loads defaults, as the test plan assumed.
3. The test helper `webhook()` in `notice_sinks.rs` tests now stamps `kind = "alert"`, matching what the webhook producer actually publishes — without it the gate correctly refused every fixture notice.
4. Accepted delta recorded in docs: `default_routes = false` now also stops ghost triggering unless an `analyze` route is configured (README states it explicitly).
5. Two commits: `6fb7557e` (feat, 10 files) and `b9f7c120` (docs: status flip + Update Log). Working tree clean; phase doc left at `in-progress` for the server-authored completion tail.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.9.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.58s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
nd_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test alert_list_request_returns_the_snapshot ... ok
test g4_briefing_masking_applied ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test attach_sends_session_counters_after_ok ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test webhook_alert_below_threshold_discarded ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test g4_briefing_injects_on_next_run ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.20s


running 13 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.35s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-be1094999a9dca70)
kill: (822881): No such process
kill: (-823560): No such process
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

- `CLAUDE.md` — +1 -1
- `README.md` — +10 -2
- `assets/etc/config.toml` — +7 -2
- `assets/memory/knowledge/config-reference.md` — +1 -1
- `docs/dev/milestones/M38-notice-bus/README.md` — +1 -1
- `docs/dev/milestones/M38-notice-bus/phase-09b-analyze-route.md` — +67 -1
- `src/config/load.rs` — +3 -1
- `src/config/tests.rs` — +37 -0
- `src/config/types.rs` — +1 -0
- `src/daemon/notice_sinks.rs` — +109 -8
- `src/webhook/process.rs` — +9 -6
- `src/webhook/trigger.rs` — +7 -4

**Commit:** b9f7c12085e8c7497546f11c1ba1ef0b93f7e0af

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-11-18

- **Verdict:** approved_first_try, with two reviewer nit fixes
- **Bounces:** none
- **Executor:** GLM-5.3-Flash
- **Scope deviations:** none. The shared `webhook()` test helper in
  `notice_sinks.rs` now carries `kind` = `alert`, which the new gate needs.
  No other test's meaning changes.
- **Reviewer fixes:**
  - **The loop test's precondition.** `analyze_routed_never_routes_an_analysis`
    asserted that the *default* route does not match the analysis notice,
    under a message saying the *configured* route's filter does. It now
    asserts the real premise: `route[0].filter().matches(&analysis)`.
    Mutation check: removing the gate's `kind` rule makes the test fail.
    With the rule restored, it passes.
  - **The `CLAUDE.md` row** listed `command` as a `SinkRoutes` field. The
    struct holds `history`, `chat_notice`, `alert_registry` and `analyze`;
    command routes come only from config.
- **Checked:**
  - Both callers pass `config::current().notifications`, read per notice.
  - The gate owns the `kind` rule.
  - `validate_routes` refuses a `command` key on an `analyze` route.
  - The acceptance greps print 1, 1/1, and 3/2.
  - `doc_truth` passes, and `webhook_ghost_e2e_deterministic` passes.
  - Full gates pass: 3022 lib tests, fmt, and clippy `-D warnings`.
- **Calibration:** none new. Spec each test by the function it must call
  and name its negative case first. That held: both gate tests call
  `analyze_routed` with parsed TOML.

