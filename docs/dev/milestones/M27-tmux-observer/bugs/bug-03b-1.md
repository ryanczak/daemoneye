# Bug 1 on phase-03b: chat drops its presence guard before the loop starts, so no push ever arrives

**Severity:** blocker
**Status:** open
**Filed:** 2026-10-10

## What's wrong

`run_chat_ratatui` (`src/cli/commands/chat.rs:313-317`):

```rust
let _presence_guard = parked_presence.as_mut().map(|(g, _)| g);
let mut pushes = parked_presence.map(|(_, rx)| rx);
```

`_presence_guard` is only a borrow, and nothing uses it after that line. The
second line moves the whole `(HeldPresenceGuard, Receiver)` tuple into the
closure. The `_` pattern does not bind the guard, so the closure drops it.
`HeldPresenceGuard::drop` then sends `done` and calls `task.abort()`. The
parked presence task dies, and the connection closes before the first
prompt.

This was confirmed with a minimal program that has the same shape. It
prints `guard dropped` before `chat loop starts`.

The consequences in a real `daemoneye chat`:

- The daemon sees EOF right away and unregisters the connection. On the
  last close it stamps detach state. So phase 03's presence is lost: the
  session is no longer spared by `cleanup_pass`, and the detach is stamped
  while the user is still chatting.
- `pushes` is a receiver whose sender has been dropped. Every alert the
  daemon broadcasts reaches no one, which defeats the goal of phase 03b.

None of the six tests catches this. `parked_presence_forwards_every_push`
calls `park()` directly. Nothing tests how the chat loop holds the guard.

### Also fix in this pass (minor, same file set)

1. **A phase-03 test was weakened.** The run deleted
   `assert_eq!(presence_count("s2"), 2, "two clients attached");` from
   `presence_with_two_clients_detaches_only_on_last`
   (`src/daemon/server/chat_presence.rs`). The spec says "all phase-03 tests
   must stay as they are". Restore the assertion.
2. **A doc comment was truncated.** The `cleanup_pass` doc comment
   (`src/daemon/session.rs`, above `pub fn cleanup_pass`) lost its last
   line, `handler locks this same store. See docs/design/daemon-stalls.md
   § 1.5c.` It now ends mid-sentence at "every IPC". Restore the line.
3. **Dead placeholder.** `noop_pushes` and `let _ = &mut noop_pushes;`
   (`chat.rs:455-457`) do nothing. Remove them.
4. **A test is not isolated.** `notify_chat_panes_pushes_over_presence`
   (`src/webhook/process.rs`) reads exactly one line after the broadcast. A
   concurrent test's global broadcast can arrive first, and then the test
   fails. This breaks the spec's isolation rule. Observe delivery by content,
   as the scheduler and server tests already do.
5. **Visibility widened.** `handle_chat_attach` was widened to `pub` and
   re-exported with `pub use`, only so that tests in the same crate can
   reach it. Use `pub(crate)` for both.
6. **The E2E evidence is missing from the Update Log.** The output went to
   `.rexymcp/output/e2e-phase-03b.txt`. The acceptance criterion requires
   the § End-to-end verification output in the Update Log entry, plus the
   line `PASTE MATCH: yes`.

## What should happen

Phase spec, Client: "Each pushed `SystemMsg` goes to the chat loop through a
channel, and the chat loop renders it." The guard has to live as long as the
chat loop. Dropping it is what ends the hold. Phase 03 already had that
contract: `drop(presence)` ran after `run_chat_inner_raw` returned.

## Root cause

The guard and the receiver are split with two `Option` combinators. The
by-value one discards the guard. The borrow made on the line before does not
keep it alive.

## Definition of done

- [ ] The split of the parked `(guard, receiver)` happens in a function that
      a test can call. New test `chat_loop_setup_keeps_presence_held` does
      the following:
      1. It parks a `HeldPresence` against a fake daemon (the
         `park_with_daemon` shape, but the daemon does not shut down).
      2. It runs that setup and keeps what it returns, as the chat loop
         would.
      3. It then has the fake daemon write a `SystemMsg`, and asserts that
         the message arrives on the receiver.

      The test reports `1 passed`. With the current two-line split, the
      message never arrives.
- [ ] `presence_with_two_clients_detaches_only_on_last` again asserts
      `presence_count("s2") == 2` after both attaches.
- [ ] `grep -n 'noop_pushes' src/cli/commands/chat.rs` prints nothing.
- [ ] `grep -c 'handler locks this same store' src/daemon/session.rs` prints `1` (today `0`): the
      truncated doc line is restored.
- [ ] `grep -nE '^pub (async )?fn handle_chat_attach|pub use chat_presence' src/daemon/server/` prints nothing.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` reports exactly `passed=2330 failed=0`: today's 2329
      plus `chat_loop_setup_keeps_presence_held`.
