# Bug 1 on phase-17c: the two "closes" idle-reap tests are flaky (timing-derived idle window)

**Severity:** major
**Status:** open
**Filed:** 2026-11-08

## What's wrong

`reap_idle_closes_a_silent_adopted_shell_at_its_prompt` (panics at
`shell_lifecycle_tests.rs:1082`) and
`reap_idle_closes_an_absent_chat_sessions_shell` (panics at `:1231`) fail
intermittently. In 12 runs of `cargo test --lib reap_idle` on the reviewer's
tree, 4 failed with `closed` empty.

## What should happen

Every named test is deterministic. STANDARDS §3 does not accept a test
that passes or fails on wall-clock timing.

## Root cause

The tests derive the idle window from spawn timing:
`silent_secs = (now - started).max(1)`, then call
`reap_idle_shells(&reg, silent_secs, now + 1)`. The reap compares against
the **cast's mtime**, which the host updates when bash draws its prompt.
That happens after `started`, often in the next second. When
`mtime = started + 1` and `now = started + 1`, the gap is
`now + 1 - mtime = 1`. That is below a `silent_secs` of 1 or 2 whenever
`now - started >= 2`, or equal to it only by luck. So the shell reads as
not idle. The test's idle precondition is never established; it is
guessed from the clock.

A related weakness: `reap_idle_skips_a_user_shell` and
`reap_idle_skips_a_shell_with_a_command_in_flight` assert a skip without
first establishing the other conditions: a `Prompt` probe and a cast old
enough to be idle. They would still pass if the owner or in-flight filter
were removed, because the shell is skipped for timing or probe reasons
instead.

## Definition of done

- [ ] Each test that expects a skip, and each that expects a close, sets
      the idle precondition explicitly and independently of spawn timing.
      For example, it backdates the cast's mtime well past the limit it
      passes. Every skip test also establishes `Foreground::Prompt` first,
      so the one condition it names is the only reason the shell is
      skipped.
- [ ] `flaky_runs=0`: 20 consecutive `cargo test --lib reap_idle` runs all
      report `9 passed; 0 failed`. **This fails against the current tree**
      (4/12 failed at review).
- [ ] Mutation, pasted: with the `Owner::User` filter removed,
      `reap_idle_skips_a_user_shell` fails. With the `in_flight` filter
      removed, `reap_idle_skips_a_shell_with_a_command_in_flight` fails.
      Restore both and paste the passing results.
- [ ] Production code (`reap_idle_shells`) is unchanged. This is a test
      fix.
