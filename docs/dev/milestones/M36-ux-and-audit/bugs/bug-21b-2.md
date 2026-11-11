# Bug 2 on phase-21b: watch_shell's starting snapshot comes from the registry, not the host

**Severity:** major
**Status:** fixed (phase-21b resume, 2026-11-11)
**Filed:** 2026-11-10

## What's wrong

`watch_shell_with` (`src/daemon/executor/knowledge/shell_watch.rs`) takes
its "already reported" snapshot from `ShellRecord::last_programs`. That is
whatever the last `list_shells` or `top` refresh happened to store, and it
is empty when neither has run. So when a program is **already** `blocked`
as the watch starts, and no listing has refreshed that shell, the watch
returns on the first poll. Behaviour 4 says it must not: the comparison is
against "the records the host held when the watch started".

`program_status_surface_watch_ignores_a_record_already_blocked_at_the_start`
passes only because it writes `last_programs` into the registry by hand
before the watch (the block between `//_prime::start` and `//prime::end`).
It pins the test's own setup, not the behaviour. Those two marker comments
are also leftover debris in `shell_observer_tests.rs`.

## What should happen

The watch reads the host's `programs` once at start, with the same probe
`program_check` uses. That reply is the baseline. The already-blocked test
doesn't touch the registry: it lets the real report land, then starts the
watch, and the watch times out.

## Root cause

The snapshot was read from the nearest stored field, not from the host,
and the test was primed to match the code.

## Definition of done

- [x] `…_watch_ignores_a_record_already_blocked_at_the_start` writes
      nothing to `last_programs` (the `prime` block and its marker comments
      are gone). It times out. The watch's baseline is the host's own
      `Status` reply, probed once at start; the test waits until the host
      really holds the record (the cast's echoed typed line lands before
      the line has executed, so a cast-keyed wait is not enough), then
      starts the watch, which times out.
- [x] The other four `…_watch_…` tests still pass.
