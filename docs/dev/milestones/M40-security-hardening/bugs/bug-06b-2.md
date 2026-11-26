# Bug 2 on phase-06b: a ghost caller's spawn is logged as user-approved

**Severity:** minor
**Status:** closed (2026-11-26, fixed in `b395cd22`; see the phase's review verdict)
**Filed:** 2026-11-26

## What's wrong

In the `SpawnGhost` arm of `dispatch_meta::dispatch`
(`src/daemon/executor/dispatch_meta.rs:137–163`), a ghost caller
(`ghost_policy` is `Some`) gets `approved = true` without a prompt. The
`ghost_spawn_approval` event is then logged unconditionally. So every
delegated spawn writes `{"event": "ghost_spawn_approval", "decision":
"approved"}` to `events.jsonl`, although no user approved anything.

The completion summary claims "a ghost caller logs none". The code does not
do that.

## What should happen

`ghost_spawn_approval` records a **user's** decision on a chat prompt. A
ghost caller sends no prompt and logs no `ghost_spawn_approval` event. The
spec's Daemon gate item logs the event on the chat prompt's two outcomes,
denied and approved. Phase 06a's `ghost_spawn_narrowed_refused` remains
the event for a ghost caller.

## Root cause

The `log_event` call sits after the `if ghost_policy.is_none() { … } else {
true }` expression, so it runs on both branches.

## Definition of done

- [ ] A new test `spawn_ghost_from_a_ghost_logs_no_approval_event`, in
      `dispatch_meta.rs`'s tests, passes.
      - **Setup:** the ghost-caller setup of
        `spawn_ghost_from_a_ghost_sends_no_prompt`.
      - **Asserts:** after `dispatch` returns, no event named
        `ghost_spawn_approval` is in the event log. Read the log with
        `crate::daemon::utils::for_each_event_between(None, None, &mut |v| …)`
        inside the same `crate::test_home()`, and match
        `v["event"] == "ghost_spawn_approval"`.
      - **Today:** it fails. The test does not exist, and the event is
        logged.
- [ ] Mutation: move the `log_event` back outside the chat branch. The new
      test fails. Restore.
