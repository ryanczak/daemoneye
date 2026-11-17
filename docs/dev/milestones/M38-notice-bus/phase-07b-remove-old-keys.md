# Phase 07b — remove-old-keys

**Milestone:** M38 — The notice bus
**Status:** in-progress
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
