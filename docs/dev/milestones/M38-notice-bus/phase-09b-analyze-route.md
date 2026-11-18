# Phase 09b — analyze-route

**Milestone:** M38 — The notice bus
**Status:** todo
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
