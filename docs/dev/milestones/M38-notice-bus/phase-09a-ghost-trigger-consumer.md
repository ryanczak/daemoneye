# Phase 09a — ghost-trigger-consumer

**Milestone:** M38 — The notice bus
**Status:** done
**Drafted:** 2026-11-18
**Depends on:** phase-08c
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

The webhook watchdog analysis — the model call that may start a ghost
shell — runs in a **queued bus consumer**, `ghost_trigger`, that takes
webhook alert notices off the bus. `process_alert` publishes the alert
and stops calling the analysis itself. The filter stays today's fixed
gate; phase 09b makes it a route.

## Architecture references

- `src/webhook/process.rs`: `process_alert` (steps 5–7),
  `maybe_analyze_alert`, `analysis_notice`.
- `src/daemon/notice_sinks.rs`: `SinkRoutes`, `default_routes`,
  `routes_match`, and the queued `chat_notice` task in
  `register_default_sinks` (the shape to copy).
- `src/webhook/server.rs` `serve`, which builds the production
  `WebhookState`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
Read **only** these files:

- Step 1: `src/daemon/notice_sinks.rs`.
- Step 2: `src/webhook/process.rs`, the new `src/webhook/trigger.rs`,
  `src/webhook/mod.rs`, and `src/webhook/server.rs`.
- Step 3: `tests/isolation.rs` (the two tests that await a ghost handle),
  and the `process_alert` / `analysis_notice` call sites in
  `tests/integration.rs`.

Step 1 ends at a green `cargo build --tests`. Step 2 ends at a green
`cargo build`; the test call sites break until step 3, which ends at a
green `cargo build --tests`.

## Spec

1. **The analyze route** (step 1).
   - `SinkRoutes` gains `analyze`.
   - `default_routes(threshold)` sets it to one filter: source `Webhook`,
     `min_severity` = threshold, statuses `Firing` and `Resolved`.
   - **Gotcha:** the statuses are load-bearing. A webhook *analysis*
     notice is source `Webhook` with status `Event` and the alert's own
     severity. Without the status list it would match, and every analysis
     would trigger another analysis.
   - Add `pub fn analyze_routed(n: &Notice) -> bool`, which matches
     `default_routes(Severity::Warning).analyze`. It is the one gate the
     producer and the consumer share.

2. **The consumer** (step 2), in the new `src/webhook/trigger.rs`.
   - `register_ghost_trigger(state: Arc<WebhookState>)` subscribes a
     queued `ghost_trigger` subscriber (capacity 256) on `state.bus` and
     spawns its task.
   - For each notice, the task runs the analysis when **all** of these
     hold:
     - `analyze_routed` matches;
     - the label `kind` is `alert`;
     - `state.config.webhook.auto_analyze` is on.
   - The task awaits each analysis before taking the next notice. A ghost
     the analysis starts stays a spawned task, and the consumer does not
     await it.
   - `serve` calls `register_ghost_trigger` once, right after it builds
     the state.

3. **The analysis reads the notice** (step 2).
   - `maybe_analyze_alert` and `analysis_notice` take the alert `Notice`
     in place of `InternalAlert`. The name is `notice.name`. The raw
     severity is the notice's `severity` label.
   - The formatted alert text is the notice's `history_text()` with its
     `[Webhook Alert] ` prefix removed.
   - `maybe_analyze_alert` no longer returns a handle.
   - Rate limiting, the cost gate, the `webhook_analysis` event and the
     ghost lifecycle notices are unchanged.
   - **Constraint:** `maybe_analyze_alert` is in
     `tests/hygiene_allowlist.txt` at 240 lines, and that count may not
     grow. If it shrinks, update the line, as the gate asks.

4. **The producer stops analysing** (step 2).
   - `process_alert` returns `()`.
   - Step 6 logs the `webhook_discarded` / `below_threshold` record when
     `analyze_routed` does not match the alert notice. The record keeps
     the same fields.
   - Step 7, the direct call to the analysis, goes.

5. **Tests wait on the bus** (step 3). The two isolation tests that
   awaited the returned ghost handle work this way instead:
   - call `register_ghost_trigger` after `rt.enter()`;
   - subscribe their own queued receiver for source `Ghost` **before**
     `process_alert`;
   - await, under a timeout, the first notice whose title starts with
     `[Ghost Shell Completed]`, `[Ghost Shell Failed]` or
     `[Ghost Shell Skipped]`;
   - make the same event-segment assertions as before.

   **Gotcha:** on a current-thread runtime a queued task spawned before
   `rt.enter()` never runs, and the wait hangs. This cost phase 03 an hour.

## Acceptance criteria

- [ ] `grep -c 'maybe_analyze_alert' src/webhook/process.rs` prints `1`
      (the definition only; `process_alert` no longer calls it).
- [ ] `grep -c 'register_ghost_trigger' src/webhook/server.rs` prints `1`.
- [ ] `grep -c 'Option<tokio::task::JoinHandle' src/webhook/process.rs`
      prints `0`.
- [ ] Every test in the Test plan exists and passes. `cargo clippy …
      -D warnings` exits 0, and `tests/hygiene.rs` passes.

## Test plan

- `notice_sinks.rs`:
  - `default_routes_reproduce_2_9_0` also asserts the analyze route:
    a webhook `Warning` firing alert and a `Warning` resolved alert match;
    a webhook `Info` alert does not.
  - `analyze_route_skips_analysis_notices`: a `Warning` webhook analysis
    notice (status `Event`, `kind` = `analysis`) does **not** match
    `analyze_routed`.
- `trigger.rs`:
  - `ghost_trigger_skips_what_it_must_not_analyse`. Publish on a test bus
    with the trigger registered and no runbook installed. None of these
    leaves a `rate_limit` entry:
    - an `Info` alert;
    - an analysis notice;
    - a ghost notice;
    - a `Warning` alert while `auto_analyze` is off.
  - `ghost_trigger_takes_a_warning_alert`: with `auto_analyze` on, a
    `Warning` alert leaves a `rate_limit` entry for its name.
  - Both tests call `register_ghost_trigger` and publish through the bus.
    Do not call the analysis directly. A test that restates the gate
    passes against a broken consumer.
- `tests/isolation.rs`: the two ghost end-to-end tests, as in spec item 5.
- `tests/integration.rs`: the `below_threshold` tests keep their
  assertions.

## End-to-end verification

The isolation ghost test (`webhook_alert` → `webhook_analysis` →
`ghost_start`) runs through the queued consumer. Paste its `cargo test`
line into the Update Log.

## Authorizations

- May touch exactly the files listed in "Edit order".
- May create `src/webhook/trigger.rs`.

## Out of scope

- The `analyze` route sink in `[[notifications.route]]`, and the docs for
  it (09b).
- The scheduled-job watchdog in `scheduled.rs`, which starts no ghost.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-18 (escalation)

**Chosen lever:** session takeover of the remainder.
**Rationale:**
- The run ended `budget_exceeded` at 1000 turns, with 4 of 5 tasks done.
  The production work (spec items 1–4) was complete and correct.
- Two test defects held it up. Each is a single mechanical fix, so a
  re-dispatch would have spent another session on a few lines.
  - `ghost_trigger_skips_what_it_must_not_analyse` called `state()` twice.
    Each call took `test_home_guard()`, a non-reentrant mutex, so the
    test deadlocked and the suite hung.
  - Both ghost end-to-end isolation tests called `register_ghost_trigger`
    **after** `process_alert` had published. A subscriber sees only what
    is published after it, so the consumer never saw the alert, and the
    5 s wait failed.
- The executor had added `DEBUG` / `RATELIMIT` / `BUSSTAT` `eprintln!`
  output, still in place, while chasing the second defect.

### Review verdict — 2026-11-18

- **Verdict:** escalated (the executor wrote the phase; the architect
  finished the remainder)
- **Bounces:** none
- **Executor:** GLM-5.3-Flash (1000 turns, `budget_exceeded`), then Claude
  (direct) for the remainder
- **Scope deviations:** none
- **Architect fixes:**
  - `trigger.rs` tests: the guard is taken once per test, and `state()`
    no longer takes it.
  - Both isolation tests register the trigger before `process_alert`.
    They wait under one overall 120 s timeout rather than 5 s per notice,
    and the debug output is gone.
  - `analysis_notice_splits_the_response` drops the webhook payload it no
    longer used. The new notice fixture had pushed `tests/integration.rs`
    past its allowlisted 3016 test lines, which may not grow.
  - A clippy `let_and_return` in the reworked test helper.
- **Checked:**
  - Mutation check: deleting the consumer's `analyze_routed` gate makes
    `ghost_trigger_skips_what_it_must_not_analyse` fail.
  - Both trigger tests publish through the bus with the consumer
    registered.
  - `maybe_analyze_alert`'s allowlist line went from 240 down to 227.
  - Acceptance greps print `1`, `1` and `0`.
  - Full gates pass: 3018 lib tests, isolation 12 passed, fmt, clippy
    `-D warnings`, hygiene.
- **Calibration:**
  - Spec item 5 said "register after `rt.enter()`" but not "before the
    publish". The ordering against the publish is the load-bearing half
    of any test that subscribes a consumer.
  - The double-guard deadlock is new data. A spec whose test builds two
    fixtures that each take `test_home_guard` should say the guard is
    taken once.

