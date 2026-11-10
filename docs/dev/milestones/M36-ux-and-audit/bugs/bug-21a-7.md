# Bug 7 on phase-21a: every output chunk re-stamps every record, so a stale working record survives into the next program

**Severity:** major
**Status:** open
**Filed:** 2026-11-10

## What's wrong

The bug-21a-6 fix made the pump call `ShellScreen::stamp_reporters(epoch)`
after **every** chunk (`src/shell/backend.rs`, the pump). That function
stamps **every record the table holds** with the current epoch. So any
output in a new line re-stamps records left over from earlier lines, and
they read as current. The comment above the call says "on every feed that
carried a report". The code does it on every feed and for every id.

The reviewer reproduced this with a throwaway test (reverted):

1. Line 1: `/bin/printf '\033]7501;state=working:id=a\007'` exits.
2. No `status` call in between.
3. Line 2: `echo hello-from-b; sleep 30`.
4. A `status` 1.5 s into the sleep returns
   `[Record { id: Some("a"), state: Working, … }]`.

That is the bug-21a-2 failure exactly. It broke again, and
`backend_status_drops_a_working_report_once_another_program_holds_the_terminal`
still passes. That test waits for the shell to go idle before it enters
line 2, and the waiting reads drop A, so it no longer pins bug-21a-2's
case. bug-21a-2's DoD required "no `status` call in between".

## What should happen

Only the records that a feed's reports actually wrote get stamped, each
with the epoch that report arrived in. A record nobody re-reported keeps
its stamp. Both of these hold:

- Line 2's output, and a read during line 2, drop A's `working` record.
  That is bug-21a-2.
- A program re-reporting the same id on line 2 is stamped with line 2's
  epoch. That is bug-21a-6.

## Root cause

`stamp_reporters` can't tell which ids the last feed reported. It walks
`table.held_ids()`. Before bug-21a-6, `stamp_missing`'s first-stamp-wins
rule hid this, because the extra stamps were no-ops. bug-21a-6 removed
that rule, so the walk overwrites every stamp. The fix needs the set of
ids **the feed reported**, not the set the table holds.

## Definition of done

- [ ] `backend_status_drops_a_working_report_once_another_program_holds_the_terminal`
      makes **no** `status` call, and doesn't wait for idle through
      `status`, between line 1 and line 2. Line 2 prints output before it
      sleeps (`echo …; sleep 30`). The read during the sleep has no record
      from line 1. A fixed delay between the lines is fine. **This test
      fails on the current tree once it is reshaped. The reviewer's probe
      of exactly this shape fails.**
- [ ] `backend_status_keeps_a_working_report_when_its_program_runs_again`
      and the rest of the phase's named tests still pass.
- [ ] The comment at the pump's stamp call matches what the code does.
