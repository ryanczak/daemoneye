# Phase 01 — daemon-alert-state

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-12
**Depends on:** none
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=fix, size=m

## Goal

GitHub issue #3. `process_alert` (`src/webhook/process.rs`) injects every
alert into every chat's history, and pushes a chat-pane notice for it,
*before* the severity gate runs. So an `info` heartbeat with
`severity_threshold = "warning"` lands in chat history every 15 minutes.

This phase does two things:
- It moves the gate ahead of history injection and the chat-pane notice.
- It gives the daemon a record of which alerts are firing. Phases 02 (chat
  status bar) and 03 (`top`) display that record. This phase ships no UI.

## Behaviour

1. **Gate first.** In `process_alert`, step 5 (`inject_into_sessions` with
   the `[Webhook Alert]` message) and step 6 (`notify_chat_panes`) run only
   when `should_fire` is true, alongside `fire_notification`. Below the
   threshold, the alert reaches the event log (`webhook_alert`, unchanged),
   the `webhook_discarded` event (unchanged), and the active-alert registry
   (behaviour 2), and nothing else. **The gate now fails closed on the
   alert's side** (PE, 2026-11-12): an alert whose severity `severity_rank`
   cannot rank is below threshold. An unrankable configured
   `severity_threshold` behaves as `warning`, and `WebhookConfig`'s
   validation warns about it at load, the way other config warnings are
   issued.
2. **Active-alert registry.** A new module `src/webhook/active.rs` holds a
   process-wide registry keyed by fingerprint, with a `_in` variant over an
   explicit map for tests, the way `presence_broadcast` /
   `presence_broadcast_in` work. Each entry holds the alert name, the
   severity string, `first_seen_unix` and `last_seen_unix`.
   - **Firing:** inserts the entry, or updates it. A re-fire keeps
     `first_seen_unix` and refreshes `last_seen_unix`.
   - **Resolved:** removes the entry with that fingerprint.
   - **Expiry:** an entry not re-seen for `ACTIVE_ALERT_TTL_SECS` (6 h) is
     pruned whenever a snapshot is taken.
   - **Snapshot:** returns entries worst severity first, then the newest
     `first_seen_unix` first. An unrankable severity ranks below `info`.
   - **Ranking:** reuse `severity_rank`. Make it `pub(crate)` and call it
     from `active.rs`. Do not copy it.
   - **Where it runs:** `process_alert` records every alert that survives
     dedup, before the gate, so a below-threshold alert is recorded. When
     a record changes the registry, the daemon broadcasts the new snapshot
     (behaviour 3).
3. **Wire.**
   - `AlertBadge { name, severity, first_seen_unix, last_seen_unix }` goes
     in `src/ipc_types.rs`.
   - `Response::AlertState { alerts: Vec<AlertBadge> }` and
     `Request::AlertList` go in `src/ipc.rs`, with their arms in its name
     match.
   - `Request::AlertList` is answered with `AlertState` holding the
     current snapshot, dispatched beside `Request::ShellList` in
     `src/daemon/server/mod.rs`.
   - `PresenceConnection.push` carries a `Response` instead of a `String`.
     `presence_broadcast(&str)` keeps its signature and wraps the text in
     `SystemMsg`. A new `presence_broadcast_frame` (with its `_in`
     variant) sends any `Response`.
   - The hold loop in `src/daemon/server/chat_presence.rs` writes each
     pushed frame as it is.
   - On `ChatAttach`, after the `Ok`, the daemon sends one `AlertState`
     frame when the registry is non-empty, and none when it is empty.
4. **Older clients are unaffected.** The client's parked presence task
   (`src/cli/commands/chat_presence.rs`) already ignores frames that are not
   `SystemMsg`. Leave it unchanged; phase 02 consumes the frame.
5. **Docs.** Update the `severity_threshold` comment in
   `assets/etc/config.toml`, its row in
   `assets/memory/knowledge/config-reference.md`, and the README's webhook
   prose to say the threshold now also gates history and the chat notice.

## Constraints

- No new config key. The TTL is the named constant.
- Tests stay hermetic: a tempdir `HOME`, as
  `webhook_alert_below_threshold_discarded` (`tests/integration.rs`) does,
  and the `_in` variants for anything process-global.
- `auto_analyze`'s write-up injection (`maybe_analyze_alert`) and
  `inject_ghost_event` are unchanged.

## Notes for executor

**Symbol map.** Every one of these exists today:
- `src/webhook/process.rs`: `process_alert`, `severity_rank`,
  `inject_into_sessions`, `notify_chat_panes`.
- `src/webhook/mod.rs`: module list.
- `src/webhook/parse.rs`: `InternalAlert`, `AlertStatus`.
- `src/daemon/session.rs`: `PresenceConnection`, `presence_broadcast_in`,
  `presence_broadcast`, `PresenceConnection::test_conn`.
- `src/daemon/server/chat_presence.rs`: the attach reply and the hold loop's
  `send_response_split(tx, Response::SystemMsg(msg))`.
- `src/ipc.rs`: `Request::ShellList`, `Response::ShellList`, the name match.
- `src/daemon/server/mod.rs`: the `Request::ShellList =>` arm.
- `tests/integration.rs`: `webhook_alert_below_threshold_discarded`.

**Edit order.** Each step ends at a green `cargo build`:
1. Write `src/webhook/active.rs` with its unit tests and register it in
   `mod.rs`. This is your first write.
2. Add `AlertBadge`, `Response::AlertState` and `Request::AlertList`, plus
   the `Request::AlertList` arm.
3. Change the presence channel to `Response` and add
   `presence_broadcast_frame`, then send the snapshot at attach.
4. Reorder `process_alert`, record the alert, and broadcast on change.
5. Add the integration tests, then the docs.

**Your first edit lands within your first 15 turns.** Never run `tmux`.
Existing presence tests that assert a frame sequence may need an empty
registry or a `_in` map. Fix the test setup, never the assertion.

## Test plan

Unit tests in `src/webhook/active.rs`, each calling the production
registry over a fresh map:
- `active_alerts_firing_inserts_and_refire_keeps_first_seen`
- `active_alerts_resolved_removes_the_entry`
- `active_alerts_snapshot_orders_worst_then_newest`
- `active_alerts_unrankable_severity_ranks_below_info`
- `active_alerts_prune_entries_past_the_ttl`

Integration tests in `tests/integration.rs`. For the history tests, a
`SessionStore` holds one entry, and the test checks its messages after
`process_alert`:
- `webhook_alert_below_threshold_is_not_injected`: `info` with threshold
  `warning`, so the entry gains no message. **It must fail on today's
  tree.** Paste that red run.
- `webhook_alert_at_threshold_is_injected`: `warning` with threshold
  `warning`, so the entry gains one `[Webhook Alert]` message.
- `webhook_alert_below_threshold_is_recorded_active`: the `info` alert
  appears in the registry snapshot.
- The two fail-open tests invert: `webhook_alert_no_severity_passes_gate`
  becomes `webhook_alert_no_severity_is_below_threshold` and
  `webhook_alert_unrankable_severity_passes_gate` becomes
  `webhook_alert_unrankable_severity_is_below_threshold`. Each asserts a
  `webhook_discarded` event with `reason: below_threshold` and no history
  message. **Both must fail on today's tree.**
- `webhook_unrankable_threshold_warns_and_acts_as_warning` (a unit test
  beside `WebhookConfig`'s validation): the warning is issued, and a
  `warning` alert fires while an `info` one does not.

Server tests:
- `alert_list_request_returns_the_snapshot`
- `presence_attach_sends_alert_state_when_alerts_are_active`, which also
  checks that no `AlertState` frame is sent when the registry is empty.
- `presence_broadcast_frame_reaches_every_connection`

## Acceptance criteria

- [ ] Every test in the test plan passes.
- [ ] `gate_first=1`: the injection call comes after `should_fire` in
      `process_alert` (command in the e2e block). Today it prints 0.
- [ ] `webhook_alert_below_threshold_discarded` still passes
      (preservation), and `grep -c 'passes_gate' tests/integration.rs`
      prints 0 (today 2).
- [ ] `grep -c 'fn severity_rank' src/webhook/active.rs` prints 0: there
      is one ranking function, not a copy.
- [ ] Mutation check, pasted in the Update Log: move the injection back
      above the gate, confirm `webhook_alert_below_threshold_is_not_injected`
      fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green. Hygiene,
      `doc_truth` and `bug_tracker` are ok.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --test integration webhook_alert 2>&1 | grep -E '^test result'
cargo test --lib active_alerts 2>&1 | grep -E '^test result'
cargo test --lib alert_state 2>&1 | grep -E '^test result'
cargo test --lib alert_list 2>&1 | grep -E '^test result'
awk '/^pub async fn process_alert/,/^}/' src/webhook/process.rs \
  | awk '/let should_fire/{g=NR} /inject_into_sessions\(&state.sessions, &alert_msg\)/{i=NR} END{print "gate_first=" (g && i>g ? 1 : 0)}'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Create `src/webhook/active.rs`.
- Edit the files in the symbol map, `src/config/types.rs`
  (`WebhookConfig` validation), plus `assets/etc/config.toml`,
  `assets/memory/knowledge/config-reference.md`, `README.md` and
  `CLAUDE.md`, if `doc_truth` asks.
- Change `PresenceConnection.push`'s type, and update its callers and
  tests.

## Out of scope

- Any client rendering: the status bar is phase 02, and `top` is phase 03.
- A config key for the TTL, or for injection policy.
- `inject_ghost_event` and the catch-up brief's prefixes.

## Update Log

<!-- entries appended below this line -->
