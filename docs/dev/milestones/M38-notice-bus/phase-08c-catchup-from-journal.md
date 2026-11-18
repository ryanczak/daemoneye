# Phase 08c — catchup-from-journal

**Milestone:** M38 — The notice bus
**Status:** todo
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
