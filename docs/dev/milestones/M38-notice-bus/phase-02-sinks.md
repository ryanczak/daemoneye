# Phase 02 — sinks

**Milestone:** M38 — The notice bus
**Status:** in-progress
**Drafted:** 2026-11-16
**Depends on:** phase-01
**Estimated diff:** ~450 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Three of today's outputs become bus consumers: chat history, the chat
notice (tmux overlay + presence push) and the `on_alert` command. The
daemon registers them on the global bus at start, with default routes
equal to what 2.9.0 delivers. Nothing publishes yet — producers move in
phases 03–05 — so runtime behaviour does not change in this phase.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design.
- `src/bus/` (phase 01): `Bus::subscribe_inline`, `Bus::subscribe`,
  `Filter`, `Notice`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Current state

The outputs exist as free functions the producers call directly:
`inject_into_sessions` and `notify_chat_panes` (`src/webhook/process.rs`,
re-exported from `crate::webhook`), `fire_notification`
(`src/daemon/utils/response.rs`), and the targeted history append in
`src/daemon/executor/bg_shell.rs` (`append_session_message` then
`push_event` on that one entry inside `with_sessions`). The daemon builds
its `SessionStore` in `run_daemon` (`src/daemon/startup.rs`). The webhook
threshold is `config.webhook.severity_threshold` (a string; unrankable
behaves as `warning`).

## Spec

1. **Notice text** — in `src/bus/notice.rs`, add
   `Notice::history_text(&self) -> String`: the title, or `title + "\n" +
   body` when the body is non-empty; and `Notice::line(&self) -> &str`:
   the title's first line. Tests `notice_history_text_joins_title_and_body`
   (both cases) and `notice_line_is_the_titles_first_line`.

2. **Routes** — new module `src/daemon/notice_sinks.rs` (register it in
   `src/daemon/mod.rs`). `SinkRoutes { history, chat_notice, command }`,
   each a `Vec<Filter>`; a notice goes to a sink when **any** of its
   filters matches (`routes_match(&[Filter], &Notice) -> bool`; an empty
   list matches nothing). `default_routes(threshold: Severity) ->
   SinkRoutes`, reproducing 2.9.0:
   - history: webhook ≥ threshold; ghost (any); background (any).
   - chat_notice: webhook ≥ threshold; ghost; scheduler; watchdog.
   - command: webhook ≥ threshold; scheduler; watchdog.
   `threshold_from_config(&str) -> Severity` is `Severity::parse`, falling
   back to `Warning`.

3. **History sink (inline)** — delivers `Message { role: "user", content:
   notice.history_text(), .. }`. `session: None` → the same set
   `inject_into_sessions` reaches (call it). `session: Some(id)` → that
   session only: `append_session_message(id, ..)`, then `push_event` on
   its entry if resident — the `bg_shell.rs` sequence. Doc comment
   constraint: **a producer must not publish while holding the
   `SessionStore` lock** — this inline sink takes it, and the bus lock is
   held across inline delivery, so that would deadlock.

4. **Chat-notice sink (queued)** — a spawned task drains its receiver and,
   per notice, runs `notify_chat_panes(&sessions, notice.line())` through
   `crate::process::off_runtime`, as `inject_ghost_event` does.

5. **Command sink (queued)** — a spawned task drains its receiver; per
   notice it reads `config::current().notifications.on_alert` (so a reload
   applies), does nothing when it is empty, and otherwise spawns `sh -c
   <cmd>` like `fire_notification`, with env `DAEMONEYE_JOB` = `name`,
   `DAEMONEYE_MSG` = `history_text()`, plus new `DAEMONEYE_SEVERITY` and
   `DAEMONEYE_SOURCE` (lowercase names). Put the env pairs in a pure
   `command_env(&Notice) -> Vec<(&'static str, String)>`.

6. **Registration** — `register_default_sinks(bus: &Bus, sessions:
   SessionStore, routes: SinkRoutes)` subscribes all three (each queued
   sink with capacity 256, subscriber names `history`, `chat_notice`,
   `command`) and spawns the two tasks. `run_daemon` calls it once, on
   `crate::bus::global()`, right after the `SessionStore` is created, with
   `default_routes(threshold_from_config(..))` from `config::current()`.
   No existing call site of `inject_into_sessions`, `notify_chat_panes` or
   `fire_notification` changes.

7. **Docs** — add a `src/daemon/notice_sinks.rs` row to CLAUDE.md's Key
   files table.

8. **Gates** — `cargo fmt --all`, `cargo clippy --all-targets
   --all-features -- -D warnings`, `cargo test`. Run each **once** when
   your edits are done; a green result ends the task — do not re-run it.

## Acceptance criteria

- [ ] Every test in the Test plan exists and passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `tests/hygiene.rs` passes with no new allowlist line.
- [ ] `grep -n "register_default_sinks" src/daemon/startup.rs` shows the
      one call.
- [ ] Preservation: `git diff --stat` shows no change to
      `src/webhook/process.rs`, `src/daemon/scheduled.rs`,
      `src/daemon/executor/bg_shell.rs` or `src/daemon/utils/response.rs`.

## Test plan

In `src/daemon/notice_sinks.rs` unless noted. Each test builds its own
`Bus::with_journal(|_| {})` — never the global bus. Tests touching
session files take `crate::test_home()` first.

- `notice_history_text_joins_title_and_body`, `notice_line_is_the_titles_first_line` (`src/bus/notice.rs`).
- `routes_match_is_any_of_and_empty_matches_nothing`
- `default_routes_reproduce_2_9_0` — with threshold `warning`, assert
  per sink: webhook `info` → no sink; webhook `warning` → all three;
  ghost `debug` → history and chat_notice, not command; background →
  history only; scheduler and watchdog → chat_notice and command, not
  history.
- `threshold_from_config_falls_back_to_warning` — `"critical"` →
  Critical; `"bogus"` and `""` → Warning.
- `history_sink_broadcast_reaches_every_resident_session` — two resident
  entries, a `session: None` notice: both gain one message whose content
  is the notice's `history_text()`.
- `history_sink_targeted_reaches_only_its_session` — two resident
  entries, `session: Some(a)`: `a` gains it, `b` does not; `a`'s session
  JSONL file holds it.
- `history_sink_skips_unrouted_notice` — a webhook `info` notice under
  the default routes leaves both entries unchanged.
- `command_env_names_the_notice` — the four keys and values.
- `command_sink_runs_on_alert_with_notice_env` (`#[tokio::test]`) —
  under `crate::test_home()`, write the temp HOME's `config.toml` with an
  `[notifications] on_alert` that writes `$DAEMONEYE_SEVERITY
  $DAEMONEYE_SOURCE` into a file in the temp HOME (uninstalled,
  `config::current()` reads that file — **never** call
  `config::handle::install` in a test); publish a scheduler `error`
  notice; poll the file up to 5 s; it reads `error scheduler`.

## End-to-end verification

Not applicable — phase ships no reachable runtime behaviour: the sinks are
registered but no producer publishes until phase 03.

## Authorizations

- May touch `CLAUDE.md` (Key files row only) and `src/daemon/startup.rs`
  (the one registration call).

## Out of scope

- Changing any producer, or removing the free functions the sinks call.
- The alert registry (moves with the webhook producer, phase 03).
- `[[notifications.route]]` config and removing the old keys (phase 07).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-16 22:21 (started, executor)

**Executor:** phase-02 run, headless.

Flipped the phase to `in-progress`; implementing the Spec tasks in order.
