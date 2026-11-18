# Phase 08c — catchup-from-journal

**Milestone:** M38 — The notice bus
**Status:** review
**Drafted:** 2026-11-18
**Depends on:** phase-08b
**Estimated diff:** ~300 lines, mostly tests
**Tags:** language=rust, kind=refactor, size=s

## Goal

The catch-up brief lists the notices from the **notice journal** over
`[detach, now)` that were routed to this chat's history **or** to the chat
notice. It no longer scans chat history for text prefixes. The header, the
away time and the cost line stay as they are.

## Architecture references

- `src/bus/journal.rs` `for_each_notice_between`.
- `src/daemon/notice_sinks.rs`: `sink_routed`, `default_routes`.
- `src/daemon/away_digest.rs` `build_away_digest`, which phase 08b moved to
  the journal the same way.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` to a source file lands within your first 15 turns.**
Read **only** these files:

- Step 1: `src/daemon/server/catchup.rs`, and `attach_brief_for` in
  `src/daemon/server/chat_presence.rs` (its call changes with the
  signature).
- Step 2: `src/daemon/server/chat_presence.rs`, only the tests named in the
  Test plan.
- Step 3: the two doc comments in `src/daemon/executor/bg_shell.rs` that
  name `build_catchup_brief`, and the `[Catch-up]` paragraph of
  `CLAUDE.md`.

Each step ends at a green `cargo build --tests`.

## Spec

1. **Select by route** (step 1). Add a pure function in `catchup.rs`. It
   takes the window's notices, the session id and a
   `&NotificationsConfig`, and returns the brief's lines in journal order.
   - **Keep a notice** when both of these hold:
     - its `session` is `None` or this session's id;
     - `sink_routed` routes it to `History` **or** to `ChatNotice`, with
       the defaults from `default_routes(Severity::Warning)`.
   - A notice routed to both sinks appears **once**.
   - **Each line** is `notice.line()`. A `Scheduler` notice's line is
     prefixed `[Scheduled] <name>: `, because its title carries no tag.

2. **Read the journal** (step 1). `build_catchup_brief` takes the session
   id, `away_secs` and `detach_time_utc`. It no longer takes the messages.
   - It streams `for_each_notice_between(detach, now)` and passes the
     notices to the function from item 1, with
     `config::current().notifications`.
   - With no `detach_time_utc`, it lists no notices.
   - The cost line reads the same `[detach, now)` window it reads today.
   - Delete the text-prefix scan and `is_bare_tag`, with the tests that
     test only them.
   - `attach_brief_for` passes the session id.

3. **Docs** (step 3). The `bg_shell.rs` comments stop claiming that the
   brief scans the `[Background Task Completed]` literal. The `CLAUDE.md`
   paragraph says the brief reads routed notices from the journal.

**Must stay:**
- The 30-second floor.
- The away-time format.
- The `[Catch-up] N event(s) while you were away (…)` header.
- The `AI activity` header for a cost-only brief.
- The cost line, `is_valid_pane_id` and their tests.
- `messages_at_detach` stays a field. Removing it is not this phase.

## Acceptance criteria

- [ ] `sed -n '/#\[cfg(test)\]/q;p' src/daemon/server/catchup.rs | grep -cE 'contains\("\[|is_bare_tag'`
      prints `0`.
- [ ] `grep -c 'for_each_notice_between' src/daemon/server/catchup.rs`
      prints at least `1`.
- [ ] Preservation: `git diff 9e2bc297 -- src/daemon/server/catchup.rs | grep -cE '^-.*(Cost during detach|AI activity while you were away|fn is_valid_pane_id)'`
      prints `0`.
- [ ] Every test in the Test plan exists and passes. `cargo clippy …
      -D warnings` exits 0.

## Test plan

The selection tests call the item-1 function over `Notice` values. **Do
not restate its rules in the test.** The window and delivery tests journal
notices the way `journal_and_replay` in the `notice_sinks.rs` tests does,
and pin each notice's instant with `Notice::ts_refire`. The record's `ts`
is the notice's own, so a fixture that sets only the record field loses it.

- `catchup.rs`:
  - `catchup_lists_what_history_and_chat_notice_route` (default config):
    - **Listed, in order:** a webhook `Warning` alert, a webhook analysis,
      a ghost notice, a background notice for this session, a scheduler
      script notice, and a watchdog notice.
    - The webhook alert, which both sinks route, appears once.
  - `catchup_skips_what_no_route_reaches_and_other_sessions`. **None** of
    these is listed:
    - a webhook `Info` alert;
    - a shell (terminal) notice;
    - a background notice for another session.
  - `catchup_honours_configured_routes`: with `default_routes = false` and
    one `history` route for source `shell`, the shell notice is listed and
    a webhook `Warning` alert is not.
  - `catchup_prefixes_scheduler_lines_with_the_job`.
  - `catchup_brief_reads_the_journal_window`: journal one notice before
    the detach and one after, then call `build_catchup_brief`. Only the
    later one is listed, under a `1 event` header.
  - The existing header, count, away-time and cost tests seed notices
    instead of messages, with the same assertions.
- `chat_presence.rs`: these four tests journal a webhook alert notice in
  place of `inject_into_sessions`, with the same assertions:
  - `attach_brief_includes_an_alert_injected_while_away`
  - `catchup_brief_reaches_a_session_that_is_not_resident`
  - `catchup_brief_names_an_alert_posted_after_the_chat_left`
  - `restart_reconnect_quit_alert_resume_briefs_the_alert`

  `restart_reconnect_alert_ask_sees_the_alert` tests history, not the
  brief. It stays as it is.

## End-to-end verification

`attach_brief_includes_an_alert_injected_while_away` reads a real journal
segment through `attach_brief_for`. Paste its `cargo test` line into the
Update Log.

## Authorizations

- May touch exactly the files listed in "Edit order".

## Out of scope

- Removing `messages_at_detach` or other detach fields.
- The away digest (done in 08b).
- **Accepted deltas** (PE decision, 2026-11-18):
  - Scheduler job results and watchdog analyses now appear in the brief.
    2.9.0 showed them only as a chat-notice overlay.
  - The `[Watch Pane` match goes; nothing produces it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-18 02:02 (started, executor)

Started phase 08c. Plan: (1) add the route-selection function in
`catchup.rs` and switch `build_catchup_brief` to the journal window;
(2) update `attach_brief_for` and the four chat-presence tests to journal
notices; (3) fix the `bg_shell.rs` doc comments and the `CLAUDE.md`
paragraph. Status flipped to `in-progress` in the phase doc and the
milestone README.

### Update — 2026-11-18 02:40 (end-to-end verification)

`attach_brief_includes_an_alert_injected_while_away` journals a webhook
alert notice into a real journal segment and reads it back through
`attach_brief_for` (the production path the attach handler calls), then
asserts the brief names the alert. Captured from the run:

```
running 1 test
test daemon::server::chat_presence::tests::attach_brief_includes_an_alert_injected_while_away ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3024 filtered out; finished in 0.00s
```

Acceptance-criterion greps (all as the spec pins):

```
$ sed -n '/#\[cfg(test)\]/q;p' src/daemon/server/catchup.rs | grep -cE 'contains\("\[|is_bare_tag'
0
$ grep -c 'for_each_notice_between' src/daemon/server/catchup.rs
3
$ git diff 9e2bc297 -- src/daemon/server/catchup.rs | grep -cE '^-.*(Cost during detach|AI activity while you were away|fn is_valid_pane_id)'
0
```

### Update — 2026-11-18 02:37 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Built phase 08c: `build_catchup_brief` now takes `(session_id, away_secs, detach_time_utc)` and streams the journal via `for_each_notice_between(detach, now)`, feeding a new pure `brief_lines` selector that keeps a notice when its session matches (or is `None`) and `sink_routed` sends it to History or ChatNotice under `config::current().notifications` with `default_routes(Severity::Warning)` defaults — a notice both sinks route appears once, each line is `notice.line()`, and `Scheduler` notices get the `[Scheduled] <name>: ` prefix. The text-prefix scan and `is_bare_tag` are deleted with their tests; the 30-second floor, away-time format, both headers, the cost line and `is_valid_pane_id` are untouched, as are `messages_at_detach` and `attach_brief_for`'s clear-on-read behavior. The four chat-presence brief tests now journal webhook alert notices through a real dated segment (pinned with `ts_refire`) instead of `inject_into_sessions`, and the `bg_shell.rs` doc comments plus the `CLAUDE.md` `[Catch-up]` paragraph describe the journal-driven brief.

Verification: `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test` all pass (3015 lib tests + all integration suites, 0 failures). All three pinned greps return the spec's values (0 / 3 / 0), and the E2E transcript for `attach_brief_includes_an_alert_injected_while_away` is pasted in the phase doc's Update Log. Two adaptations worth noting: an `Info`-severity `[Webhook Analysis]` is *not* listed under the defaults (the default webhook route is severity ≥ `warning`, which the spec's own fixture list matches by giving the analysis notice a `Warning` severity in `catchup_lists_webhook_analysis`), and the digest-join test's brief now journals its notice inside a detach window pinned to the digest's stamp rather than wall-clock — otherwise the brief came back empty once the brief went journal-based. Working tree is clean; status left at `in-progress` for the server's completion tail.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.9.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.55s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
nd_trip ... ok
test ghost_config_parsing ... ok
test config_pricing_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test alert_list_request_returns_the_snapshot ... ok
test attach_sends_session_counters_after_ok ... ok
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
test webhook_alert_without_sinks_reaches_no_history ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.19s


running 13 tests
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
test daemon_start_replays_active_alerts ... ok

test result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.37s


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
kill: (696270): No such process
kill: (-696926): No such process
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
- `docs/dev/milestones/M38-notice-bus/README.md` — +1 -1
- `docs/dev/milestones/M38-notice-bus/phase-08c-catchup-from-journal.md` — +36 -1
- `src/daemon/executor/bg_shell.rs` — +8 -6
- `src/daemon/server/catchup.rs` — +417 -170
- `src/daemon/server/chat_presence.rs` — +132 -65

**Commit:** b102334756c49427c601e5acd7fe7270d673eeb6

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
