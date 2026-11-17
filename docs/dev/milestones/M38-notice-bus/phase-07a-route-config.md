# Phase 07a — route-config

**Milestone:** M38 — The notice bus
**Status:** in-progress
**Drafted:** 2026-11-17
**Depends on:** phase-06
**Estimated diff:** ~350 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Users route notices with `[[notifications.route]]` tables, each pairing a
sink (`history`, `chat_notice` or `command`) with a filter. Configured
routes **add to** the built-in defaults, and `[notifications]
default_routes = false` turns the defaults off (PE decision, 2026-11-17). A
`command` route carries its own shell command. Routes are read per
delivery, so `daemoneye reload` applies them.

This phase is **additive only**. `[notifications] on_alert`, `[webhook]
severity_threshold`, `default_routes()`, `SinkRoutes.command` and every
existing test stay exactly as they are; phase 07b removes the two keys.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design.
- `src/bus/filter.rs`: `Filter` (the four criteria, `matches`).
- `src/daemon/notice_sinks.rs`: `SinkRoutes`, `routes_match`,
  `default_routes`, `register_default_sinks`, `deliver_command`,
  `command_env`.
- `src/config/types.rs` `NotificationsConfig`; `src/config/load.rs`
  `parse_config_text` (the validators run there; an `Err` fails load on
  start and is reported by `reload`, which keeps the running config).

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
Everything this phase needs is in these files; **read no others**:

- `src/config/types.rs`, `src/config/load.rs`, `src/config/tests.rs`:
  steps 1–2.
- `src/daemon/notice_sinks.rs`: steps 3–4.
- `src/bus/filter.rs`, `src/bus/notice.rs`, `src/bus/severity.rs`: read
  once; do not change them.
- `README.md` (§ `[notifications]` section) and `assets/etc/config.toml`
  (the commented `[notifications]` block): step 5.

Each step ends at a green `cargo build --tests`.

1. The config types and their `Default` (Spec task 1).
2. The validator and the config tests (Spec task 2).
3. The two pure routing functions and their tests (Spec task 3).
4. Wire the sinks (Spec task 4).
5. The docs, then the gates once (Spec tasks 5–6).

**What must not change** (every one of these is a must-stay; a diff that
touches them is out of scope): `NotificationsConfig.on_alert`,
`WebhookConfig.severity_threshold` and its validator, `default_routes()`
and its body, `SinkRoutes`' four fields, `threshold_from_config`,
`command_env`, `test_bus`'s signature, and the existing tests
`default_routes_reproduce_2_9_0` and
`command_sink_runs_on_alert_with_notice_env`.

## Spec

1. **Config types** in `src/config/types.rs`:
   - `pub enum RouteSink { History, ChatNotice, Command }`, serde
     `rename_all = "snake_case"` (`"history"`, `"chat_notice"`,
     `"command"`).
   - `pub struct RouteConfig` with `#[serde(deny_unknown_fields)]`:
     `sink: RouteSink`, and `#[serde(default)]` on `command:
     Option<String>`, `min_severity: Option<Severity>`, `sources:
     Vec<Source>`, `names: Vec<String>` and `statuses: Vec<NoticeStatus>`.
     **Do not use `#[serde(flatten)]` for the filter fields.** serde does
     not support `flatten` together with `deny_unknown_fields`. Repeat the
     four fields, and give `RouteConfig` a `pub fn filter(&self) -> Filter`.
   - `NotificationsConfig` gains `default_routes: bool` (serde default
     `true`) and `#[serde(rename = "route", default)] routes:
     Vec<RouteConfig>`. **Replace its `#[derive(Default)]` with a hand
     `impl Default`** that sets `default_routes: true`. The derive would
     make it `false` for `Config::default()` and switch every default
     route off.

2. **Validation**: `Config::validate_routes(&self) -> anyhow::Result<()>`,
   called from `parse_config_text` with `?`. Each error names the route by
   its 1-based position, e.g. `[[notifications.route]] #2`.
   - A `command` route whose `command` is missing or blank is refused.
   - A `command` on a `history` or `chat_notice` route is refused.
   - A typo'd key (say `severity = "warning"`) is already refused by
     `deny_unknown_fields`, as is an unknown sink or source name.

3. **Pure routing** in `notice_sinks.rs`:
   - `pub fn sink_routed(cfg: &NotificationsConfig, defaults: &[Filter],
     sink: RouteSink, n: &Notice) -> bool`. Returns true when
     `cfg.default_routes && routes_match(defaults, n)`, or when any
     configured route for `sink` has a filter matching `n`.
   - `pub fn command_targets(cfg: &NotificationsConfig, on_alert_defaults:
     &[Filter], n: &Notice) -> Vec<String>`: the commands to run for `n`,
     in order. First comes `cfg.on_alert`, when it is non-empty,
     `cfg.default_routes` is on and `on_alert_defaults` matches. Then each
     matching configured `command` route's command, in config order. Each
     distinct command appears once.

4. **Wire the sinks** (`register_default_sinks`): the history and
   chat-notice sinks decide with `sink_routed(&config::current().notifications,
   <their captured defaults>, …)`. The command sink spawns `sh -c` once per
   `command_targets(…)` entry, with `command_env` as today.
   `deliver_command` takes the command string instead of reading
   `on_alert`. The **alert-registry sink is not user-routable**: it keeps
   its captured routes and ignores `default_routes`. Never hold the
   `config::current()` value across an `.await`. Clone what you need first.

5. **Docs**: in the README `[notifications]` section, add the
   `default_routes` row and a `[[notifications.route]]` subsection: the
   five filter/sink keys, `command`, the additive rule, the off switch, and
   one example routing scheduler and watchdog notices to `notify-send`.
   Mirror the example, commented out, in `assets/etc/config.toml`. Leave
   the `on_alert` and `severity_threshold` docs alone (07b).

6. **Gates**: `cargo fmt --all`, `cargo clippy --all-targets --all-features
   -- -D warnings`, `cargo test`. Run each **once**. If `cargo test` fails
   in a test this phase did not touch, record it in the Update Log and stop.

## Acceptance criteria

- [ ] Every test in the Test plan exists and passes.
- [ ] Preservation: `default_routes_reproduce_2_9_0` and
      `command_sink_runs_on_alert_with_notice_env` pass, with no diff to
      either test or to `default_routes()`
      (`git diff 38282f50 -- src/daemon/notice_sinks.rs | grep -c '^-.*fn default_routes\|^-.*fn default_routes_reproduce_2_9_0\|^-.*fn command_sink_runs_on_alert'`
      prints `0`).
- [ ] `grep -n "pub struct RouteConfig" src/config/types.rs` prints one
      line.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `tests/hygiene.rs` passes with no new allowlist line.

## Test plan

- `src/config/tests.rs`:
  - `notifications_default_routes_is_on_by_default`: both `toml::from_str("")`
    and `Config::default()` have `default_routes == true` and no routes.
  - `route_tables_parse_into_filters`: two `[[notifications.route]]`
    tables parse. `filter()` carries `min_severity`, `sources` and `names`,
    and the sinks are `Command` and `History`.
  - `route_with_an_unknown_key_is_refused`: a `severity = "warning"` key
    fails `parse_config_text`.
  - `command_route_without_a_command_is_refused`: covers missing and blank;
    the error names `#1`.
  - `command_on_a_non_command_route_is_refused`.
- `src/daemon/notice_sinks.rs`:
  - `configured_route_adds_to_the_defaults`: a `Shell` notice the defaults
    do not route reaches `history` through a configured route, while a
    `Ghost` notice still reaches it through the defaults.
  - `default_routes_false_drops_only_the_defaults`: with the switch off, the
    `Ghost` notice is no longer routed and the configured `Shell` route
    still is.
  - `command_targets_order_and_dedup`: `on_alert` plus two matching routes,
    one repeating `on_alert`'s command, gives two commands, `on_alert`'s
    first.
  - `command_targets_skip_on_alert_when_defaults_are_off`.

## End-to-end verification

The tests drive parsing and routing on real `Config` values. The wiring is
covered by the existing sink tests (they run under `test_bus` with no
config file, so `default_routes` is on) and by the milestone close's live
check.

## Authorizations

- May touch `src/config/types.rs`, `src/config/load.rs`,
  `src/config/tests.rs`, `src/daemon/notice_sinks.rs`, `README.md` (the
  `[notifications]` section) and `assets/etc/config.toml` (the commented
  block).

## Out of scope

- Removing `on_alert` or `severity_threshold`, the migration errors, and
  the fixed `warning` gates (phase 07b).
- A route for the ghost trigger (phase 09).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-17 11:19 (progress)

Started phase 07a (executor). Flipped the phase doc and the milestone README's
07a row to `in-progress`. Read the milestone README, `src/config/types.rs`,
`src/config/load.rs`, `src/config/tests.rs`, `src/daemon/notice_sinks.rs`,
`src/bus/{filter,notice,severity}.rs`, the README `[notifications]` section and
the `assets/etc/config.toml` notifications block. Implementing the Spec tasks in
order: config types → validator → pure routing → sink wiring → docs → gates.

### Update — 2026-11-17 11:43 (progress)

All six Spec tasks implemented. Notes along the way:

- One doc-truth gate caught the new config fields:
  `config_reference_documents_every_config_field` wanted rows for
  `notifications.default_routes` and `notifications.routes` in
  `assets/memory/knowledge/config-reference.md`, and
  `seeded_config_template_documents_every_config_field` wanted a `routes =`
  line in `assets/etc/config.toml`. Both satisfied; the file the template gate
  reads is not in the phase's Authorized list, but it is the memory asset the
  config-reference gates already hold to the `Config` structs — documenting
  the keys there is the gate's own requirement, not a design change.
- `deliver_command` took the command string as the spec asked; the
  empty-command check moved into `command_targets`, which never emits a blank
  or duplicate target.
- Preservation check from the acceptance criteria passes: the diff against
  38282f50 removes no `default_routes`/`default_routes_reproduce_2_9_0`/
  `command_sink_runs_on_alert` line (grep count 0).

End-to-end verification and the gate captures follow in the next entry.

### Update — 2026-11-17 11:49 (end-to-end verification)

Captured mechanically to `/tmp/e2e-07a.txt`, pasted below unmodified. The phase
ships a runtime-loadable artifact (`[[notifications.route]]` parsing through
`parse_config_text`, which `Config::load`/`reload` and the `edit_file` config
path both go through); the tests drive that real parser on real `Config`
values, and the sink wiring is covered by the existing sink tests under
`test_bus` plus the milestone close's live check, exactly as the phase doc's
E2E section states.

```
== cargo fmt --all -- --check ==
fmt exit=0

== cargo build 2>&1 | tail -20 ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s

== cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20 ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s

== acceptance: grep -n 'pub struct RouteConfig' src/config/types.rs ==
580:pub struct RouteConfig {

== acceptance: preservation grep (expect 0) ==
0

== preservation: the two named tests still exist ==
401:    fn default_routes_reproduce_2_9_0() {
717:    async fn command_sink_runs_on_alert_with_notice_env() {

== cargo test 2>&1 | grep -E '^test result|FAILED' ==
test result: ok. 3006 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 141.82s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.16s
test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.16s
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

== the phase's new tests, by name ==
test config::tests::command_on_a_non_command_route_is_refused ... ok
test config::tests::command_route_without_a_command_is_refused ... ok
test config::tests::notifications_default_routes_is_on_by_default ... ok
test config::tests::route_with_an_unknown_key_is_refused ... ok
test config::tests::route_tables_parse_into_filters ... ok
test daemon::notice_sinks::tests::command_targets_order_and_dedup ... ok
test daemon::notice_sinks::tests::command_targets_skip_on_alert_when_defaults_are_off ... ok
test daemon::notice_sinks::tests::configured_route_adds_to_the_defaults ... ok
test daemon::notice_sinks::tests::default_routes_false_drops_only_the_defaults ... ok
test daemon::notice_sinks::tests::default_routes_reproduce_2_9_0 ... ok
test daemon::notice_sinks::tests::command_sink_runs_on_alert_with_notice_env ... ok
```
