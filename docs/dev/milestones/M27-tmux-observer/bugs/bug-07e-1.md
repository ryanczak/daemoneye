# Bug 1 on phase-07e: an alert posted after the chat left never reaches the history, so the brief is empty

**Severity:** major
**Status:** open
**Filed:** 2026-10-12

## What's wrong

Live S2 re-run against `c93ae1ed`. The steps: the daemon restarted, the chat
reconnected, and the chat quit without asking. Then a `LiveTicks` alert was
posted 38 s later (daemon.log `01:58:21 Webhook alert: 'LiveTicks'`), and
`chat --resume` followed. No `[Catch-up]` appeared, and the session file
holds no `[Webhook Alert]` (`grep -c` → 0).

`inject_into_sessions` (`src/webhook/process.rs`) writes to the union of the
store's keys and `presence_session_ids()`. At 01:58:21 the session was in
neither: it was not resident (the restart), and its presence had already
been unregistered (the quit). So the alert was written to no session, and
07e's non-resident brief had no new messages to report.

## What should happen

An alert posted while a chat is away is in that session's history, and the
catch-up brief on resume names it. This holds whether or not the session
is resident.

## Root cause

07e records a non-resident detach (`NON_RESIDENT_DETACHES`), but the inject
path does not treat a recorded detach as a target. The pinned test
`catchup_brief_reaches_a_session_that_is_not_resident` kept its presence
connection registered while it injected, which a real detach never does
(`handle_chat_attach` unregisters, then calls `record_chat_detach`). That
was the architect's spec wording, so the test passed on an order that cannot
occur in production.

## Definition of done

- [ ] `catchup_brief_names_an_alert_posted_after_the_chat_left` passes. It
      fails against the current tree, because the test does not exist yet.
- [ ] `cargo test` reports `passed=2325 failed=0`.
