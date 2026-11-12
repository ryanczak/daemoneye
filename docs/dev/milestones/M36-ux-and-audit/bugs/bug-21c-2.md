# Bug 2 on phase-21c: `working` doesn't start when the turn is sent, and an error return skips `idle`

**Severity:** major
**Status:** fixed (phase-21c re-dispatch, 2026-11-12)
**Filed:** 2026-11-12

## What's wrong

1. **No `working` at send.** Behaviour 1 says the state is `working` "from
   the moment a turn is sent". `ask_with_session_ratatui`
   (`src/cli/commands/stream_ask.rs`) never calls `Emitter::working`
   directly. The first report is driven by the first `Token`,
   `ToolStarted` or `ToolResult` frame. So while the model is thinking
   before its first token (tens of seconds on a large context), the
   terminal still reads `idle`.
2. **Error paths skip `idle`.** `idle` is reported on the normal end and on
   the two `StreamOutcome` error arms. The function also returns early
   through `?`: `connect().await?`, the request send, and every
   `send_request(…).await?` after a prompt. Those returns skip it, so a
   failed reply can leave the terminal reading `working`, or `blocked`
   until exit. `Emitter::idle_on_drop` (`program_status_emit.rs:233`) was
   written for exactly this case, but nothing calls it.

## What should happen

`working` is reported when the turn's request is sent. `idle` is reported
on **every** return from `ask_with_session_ratatui`, early `?` returns
included. A drop guard is the natural shape, and it already exists.

## Root cause

The wiring only touched the frame loop and the two visible returns. The
turn-start transition and the `?` exits were not covered.

## Definition of done

- [ ] `grep -c 'idle_on_drop' src/cli/commands/stream_ask.rs` prints 1 or
      more, and the hand-placed `status.idle()` calls it makes redundant
      are gone. Today 0.
- [ ] `grep -c 'status.working()' src/cli/commands/stream_ask.rs` prints 1
      or more, at the point the turn's request is sent. Today 0.
- [ ] `program_status_emit_idle_on_drop_reports_idle` passes: a guard
      dropped after `working` leaves the writer's last report `idle`.
