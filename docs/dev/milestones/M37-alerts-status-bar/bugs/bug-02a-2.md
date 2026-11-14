# Bug 2 on phase-02a: bug-02a-1's reply fix never landed, and its tests test nothing

**Severity:** major
**Status:** open
**Filed:** 2026-11-14

## What's wrong

The bug-02a-1 re-dispatch reported `complete` after 817 turns, but:

1. **The reply transition is not in the tree.**
   `grep -c 'ChatState::Working' src/cli/commands/stream_ask.rs` still
   prints 0, and `stream_ask.rs` has no diff. The run's last action was a
   script to re-apply the nine transitions after a mutation check, and it
   never ran. So the bar still shows `AwaitingApproval` from the first
   approval until the turn ends.
2. **Two of the new tests are tautologies.**
   `the_reply_transition_returns_the_bar_to_working` (`stream.rs`, end of
   the tests module) assigns `sb.state = Working` itself, then asserts it.
   `the_reconnected_push_returns_reconnecting_to_idle` (`chat_presence.rs`)
   copies the loop's `if`/`else if` into the test body. Neither calls
   production code, so deleting the production lines leaves both green.
3. **A failed turn still ends `Idle`, not `DaemonDown`.** The turn ask's
   `Err` arm (`chat_loop.rs:380–382`) calls `sb.set_daemon_up(false)`
   while the state is `Working`, and `set_daemon_up` only demotes `Idle`
   (`status_bar.rs`, `set_daemon_up`), so the call is a no-op. Four lines
   later (`chat_loop.rs:~388`) the turn end sets `Idle` unconditionally.
   Only the startup ask's arm (`chat_loop.rs:136`, reached while `Idle`)
   works.
4. No mutation check is pasted in the Update Log, and the work is
   uncommitted.

The real tests in `status_bar_tests.rs` (`set_daemon_up_*`) are fine. Keep
them, and keep the `chat_loop.rs` changes.

## What should happen

As bug-02a-1 states: `Working` again on each prompt's reply, `DaemonDown`
after *any* failed ask until the next success, and `Idle` on the
`"reconnected"` push. Each transition is pinned by a test that **calls the
production code**, so removing the production transition fails the test.

## Root cause

The fix and its tests were written separately: the tests restate the
intended assignment instead of exercising the code that makes it. With the
transitions as bare assignments inside large `async` arms, there was
nothing callable to test, so the tests restated the assignment and passed
regardless.

## Definition of done

- [ ] `grep -c 'ChatState::Working' src/cli/commands/stream_ask.rs` prints
      0 today. After the fix, every `status.reply()` arm returns the model to
      `Working` (directly or through one `StatusModel` method they all call).
- [ ] The reply, failed-turn and reconnected transitions each live in a
      function production calls, for example a `StatusModel` method or a
      small pure fn the loop's arm calls. Each has a test that calls that
      same function.
      `the_reply_transition_returns_the_bar_to_working` and
      `the_reconnected_push_returns_reconnecting_to_idle` are rewritten or
      deleted so they call it.
- [ ] A failed **turn** ask leaves the state `DaemonDown` after the turn
      ends, not `Idle`. Pinned by a test of the turn-end function.
- [ ] Mutation check pasted with the failing `test result:` line: delete the
      reply transition, the reply test fails; revert.
- [ ] Committed. fmt, build (0 warnings), clippy and `cargo test` are green.
