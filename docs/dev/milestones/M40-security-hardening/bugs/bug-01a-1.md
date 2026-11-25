# Bug 1 on phase-01a: three of the new socket tests prove less than they claim

**Severity:** minor
**Status:** closed (2026-11-25, architect takeover; see the phase's review verdict)
**Filed:** 2026-11-25

## What's wrong

1. **`accept_bounded_waits_for_a_free_permit` waits on nothing.** It
   connected the second client only *after* the 200 ms wait. With no
   connection pending, `accept` blocks on its own, so `!second.is_finished()`
   holds whatever the permit does. The spec ordered it "hold the first
   connection's permit, **then connect a second client**", and the
   executor's completion note says it reordered the steps because its first
   version timed out. The test's `available_permits() == 0` assertion still
   catches a semaphore that is ignored entirely.
2. **`accept_bounded_waiting_call_can_be_dropped` finds its mutation only
   by hanging.** The final `accept_bounded` had no timeout. The reviewer
   moved `acquire_owned` after `accept`, and the test then hung past 60 s
   rather than failing. The spec's constraint was "every await in the new
   socket tests runs under a timeout".
3. **`daemon_refuses_an_oversized_request_within_budget` ends with a
   tautology:** `assert!(handle.await.unwrap().is_err() || true, …)`.
   Neither budget test bounds its `write_all` loop with a timeout.

## What should happen

Each test fails, never hangs, under the mutation it exists to catch, and
no assertion is true by construction.

## Root cause

The executor reordered a step the spec had ordered, to work around a
timeout it did not diagnose. The `|| true` most likely papered over a
`handle_client` result it was unsure of. `handle_client` returns `Ok(())`
after sending the refusal.

## Definition of done

- [x] Mutation: take the permit after `accept`. `accept_bounded_waiting_call_can_be_dropped`
      **fails** within about 2 s (it hung before).
- [x] Mutation: ignore the shared semaphore. Both accept tests fail.
- [x] The daemon test asserts that `handle_client` returned `Ok`, under a
      timeout. Every `write_all` in both budget tests is under a 5 s
      timeout.
- [x] All four gates pass. The lib suite reports 3125 passed.
