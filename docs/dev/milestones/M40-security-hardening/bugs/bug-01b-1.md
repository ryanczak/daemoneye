# Bug 1 on phase-01b: the re-pointed hold-error test fails about 45% of runs

**Severity:** major
**Status:** closed (2026-11-25, architect takeover; see the phase's review verdict)
**Filed:** 2026-11-25

## What's wrong

`presence_unregisters_when_the_hold_ends_in_error` (`src/daemon/server/chat_presence.rs`)
was re-pointed by the phase. Its old trigger, non-UTF-8 input, no longer
errors under `read_bounded_line`. The executor disclosed the change in its
notes. The new version drops the client's read half, broadcasts a push,
then drops the write half, and asserts the hold ends in `Err`.

Run alone, it failed the first time, and **136 of 300** runs failed (16 in
parallel) with `the hold ends in an I/O error: Ok(())`. It passed in both
full-suite runs by chance.

## What should happen

The test reaches the hold's error path on every run.

## Root cause

`#[tokio::test]` runs on a current-thread runtime, so the hold task does
not run until the test awaits it. By then both client halves are dropped,
so **two** `select!` branches are ready at once:
- the read, which sees EOF and returns `Ok(())`;
- the queued push, whose write fails and returns `Err`.

`tokio::select!` picks between ready branches at random. The test's
comment, "the push is broadcast after the drop so it is the write that
fails", assumes an order that `select!` does not keep.

## Definition of done

- [x] Stress loop: 300 runs, 16 in parallel, of the lib binary `--exact`
      on the test. **0** failures (136 before).
- [x] The test still asserts `Err`, the unregister and the `last_detach`
      stamp. The hold has exactly one ready branch: a test reader fails
      on demand while both client halves stay open.
