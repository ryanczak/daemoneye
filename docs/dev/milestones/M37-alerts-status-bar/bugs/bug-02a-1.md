# Bug 1 on phase-02a: the bar's state slot drifts from the emitter

**Severity:** major
**Status:** fixed (architect takeover, 2026-11-14)
**Filed:** 2026-11-14

## What's wrong

Behaviour 2 says the `ChatState` is "set at the points the emitter is
driven, not inferred a second time", with `Reconnecting` coming from the
presence push and `DaemonDown` from `daemon_up`. Three of those transitions
are missing:

1. **`AwaitingApproval` sticks for the rest of the turn.** In
   `src/cli/commands/stream_ask.rs`, each of the nine approval arms sets
   `sb.state = ChatState::AwaitingApproval` next to `status.blocked(…)`
   (for example at lines 317–319). But the matching `status.reply()`
   (lines 341, 349, 357, 386, 424, 440, 467, 501, 539) puts the *emitter*
   back to `working` and leaves `sb.state` alone.
   `grep -c 'ChatState::Working' src/cli/commands/stream_ask.rs` prints 0.
   After the first approval in a turn, the bar keeps showing the awaiting
   glyph, and drops the `⏱` elapsed seconds, while the approved command
   runs. It recovers only when `chat_loop.rs:384` sets `Idle` at turn end.
2. **`DaemonDown` is never reached.** Nothing calls `set_daemon_up(false)`
   (`grep -rn 'set_daemon_up(false)' src/cli` finds nothing). The startup
   ask's `Err` arm (`src/cli/commands/chat_loop.rs:135`) and the turn
   ask's `Err` arm (`chat_loop.rs:377`) only print. The model starts at
   `Idle` (`StatusModel::default`), so a chat started with the daemon down
   shows the idle glyph. Before this phase, `daemon_up` started `false`
   and went `true` only on a successful ask.
3. **`Reconnecting` is never cleared by the reconnect.** The push arm
   (`chat_loop.rs:436–440`) sets `Reconnecting` on "daemon connection
   lost", but the presence task's `"reconnected"` push
   (`chat_presence.rs:155`) leaves it there until the next turn ends.

## What should happen

The bar's state follows the same transitions the emitter reports:
`Working` from the send until the turn ends, `AwaitingApproval` only while
a prompt is open, and `Working` again on its reply. `DaemonDown` applies
whenever an ask could not reach the daemon and clears on the next
successful one. `Reconnecting` returns to `Idle` on the `"reconnected"`
push.

## Root cause

Phase 02a's wiring mirrored `status.working()` and `status.blocked()` onto
the model but not `status.reply()`, and it dropped the old `daemon_up`
bool without carrying its `false` path into `set_daemon_up`. The
`status_line_*` tests pin the pure line for each state, but nothing pins
the transitions, so all four gates stayed green.

## Definition of done

- [ ] After an approval prompt's reply, the model's state is `Working` and
      `turn_started_at` is unchanged. Pinned by a unit test that drives the
      transition through the same code the stream arms call. Confirmed
      failing against today's tree by construction: no such transition
      exists (`grep -c 'ChatState::Working' src/cli/commands/stream_ask.rs`
      prints 0 today).
- [ ] A failed ask leaves the state `DaemonDown`, and the next successful
      ask leaves it `Idle`. Pinned by a test, and `grep -rn
      'set_daemon_up(false)' src/cli` (or an equivalent call) is non-empty;
      today it is empty.
- [ ] The `"reconnected"` push returns `Reconnecting` to `Idle`, pinned by a
      test.
- [ ] Mutation check pasted in the Update Log: remove the reply transition,
      confirm the reply test fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green.
