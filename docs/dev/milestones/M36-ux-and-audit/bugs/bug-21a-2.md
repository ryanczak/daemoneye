# Bug 2 on phase-21a: a working record outlives its program when another one starts

**Severity:** major
**Status:** open
**Filed:** 2026-11-09

## What's wrong

Behaviour 3 drops a `working` or `blocked` record when **either** of two
things happens: the shell is back at its own prompt, **or a different
process group is in the foreground**. The phase also says "a read must
never return a stale `working` record."

`PtyBackend::status` (`src/shell/backend.rs`) expires records only when no
job holds the terminal (`!holding`), and only at the moment of the read. Take
this sequence:

1. Command A prints a `working` report and exits.
2. Command B starts before anything calls `status`.

A read during B returns A's `working` record. The host never noticed that A
left the foreground, because B holds it now. Nothing records which process
group a report came from.

## What should happen

Each `working` or `blocked` record remembers the foreground process group
that held the terminal when it was reported. A read drops it when the
terminal is held by a different group, or by the shell itself. Records
reported while the shell itself held the terminal are dropped as soon as a
read finds any other state, or the same rule applies to them. Pick one and
pin it in a test. `idle`, `done` and `error` keep their current behaviour.

## Root cause

The lifetime check was implemented as "is anything in front?" rather than
"is the reporter still in front?". The report path never captures the
foreground group.

## Constraints

- The lock order in the phase doc still applies. `status()` takes `shell`
  and then `screen`. The pump must not lock `shell` while it holds
  `screen`. Stamping the group after the feed, with the screen lock
  released, satisfies this.

## Definition of done

- [ ] `backend_status_drops_a_working_report_once_another_program_holds_the_terminal`
      passes. Use real bash: one command prints `working` and exits, and a
      second command (`sleep`) starts with no `status` call in between. A
      `status` during the sleep has no `working` record. **This test
      doesn't exist yet. Written against the current tree, it fails.**
- [ ] `backend_status_carries_a_working_report_while_its_program_runs` and
      `backend_status_keeps_a_done_report_after_its_program_exits` still pass.
