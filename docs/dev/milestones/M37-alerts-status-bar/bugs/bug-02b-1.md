# Bug 1 on phase-02b: the attach and the 1 s task keep separate last-pushed records

**Severity:** major
**Status:** fixed (phase-02b re-dispatch, 2026-11-14)
**Filed:** 2026-11-14

## What's wrong

Spec 3 and 4 describe **one** last-pushed value per session. The 1 s task
diffs against it, the attach's value becomes it, and the last close
forgets it. The tree keeps two records:

- `session_counters_loop` (`src/daemon/startup.rs:24`) builds its own
  `CountersTracker::default()` and passes it to `counters_tick`.
- The attach (`src/daemon/server/chat_presence.rs:110`) goes through
  `record_session_counters`, which reads and writes the process-global
  `counters_tracker()` (`src/daemon/counters.rs:121`). The last-close path
  (`chat_presence.rs:151`) forgets from that global one only.

Consequences:

1. **A second connection gets no counters at attach.**
   `record_session_counters` pushes only when the global record differs
   (`wants_push`). Nothing but an attach ever writes that record, so a
   second chat attaching to a session whose counts haven't changed since
   the first attach gets no `SessionCounters`. Its bar shows zero
   background and zero shells until something changes. Spec 4 says the
   hold sends one unconditionally.
2. **The task's record is never forgotten.** It keeps one entry for every
   session id that was ever present, for the daemon's lifetime, and after
   a re-attach it diffs against a stale value.

## What should happen

The task, the attach and the last-close forget share one per-session record
(spec 3 and 4). The attach always delivers one `SessionCounters` to the
connection that just attached, after its `Ok` (and after its `AlertState`,
when one is sent), and records that value. The task pushes only on a change
from that shared record, and the last close forgets it.

## Root cause

`counters_tick` takes `&mut CountersTracker`, and the startup loop handed it
a fresh local one instead of the global one. Separately,
`record_session_counters` folds "should I push?" into the attach path, which
must always send.

## Definition of done

- [ ] `grep -c 'CountersTracker::default()' src/daemon/startup.rs` prints 0
      (today 1). The task drives the same record the attach writes and the
      last close forgets.
- [ ] A test (name it
      `a_second_connection_gets_session_counters_at_attach`) attaches two
      connections to one session with unchanged counts. Both receive a
      `SessionCounters` frame after their `Ok`. It fails on today's tree.
- [ ] A test pins that after an attach records a value, one `counters_tick`
      pass with the same counts pushes nothing, and that after the last
      close's forget, the next tick pushes again. It calls the production
      tick and forget, not a copy of them.
- [ ] Mutation check pasted with its `test result:` line: make the attach
      skip the send when the record matches, and confirm the second-connection
      test fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green.
