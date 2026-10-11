# Bug 1 on phase-06d: the audit-log sample was rewritten to a record format the code does not write

**Severity:** major
**Status:** open
**Filed:** 2026-10-11

## What's wrong

`README.md`, the audit-log section (~1427–1432). This was edited outside
the work list:

> `[1748000000] session=abc123 shell=s1 status=approved cmd=ps aux …`
> Fields for command records: Unix timestamp · session ID · shell id · …

No command record carries a shell id. `log_command`
(`src/daemon/utils/event_log.rs`) writes a `command` event with the fields
`session`, `mode`, `pane`, command, status and output excerpt. Its callers
still pass `"foreground"` / `"background"` as `mode`. The rewrite is
inferred, not read from the code: the same failure class as bug-06b-1.
A user parsing their audit log for a `shell` field would find none.

## What should happen

The work list's rule: "Edit only these rows. Anything else stale goes in the
Update Log." The audit-log sample was not a row.

## Root cause

The executor judged the sample stale (it names foreground and background)
and rewrote it to what it guessed a PTY-only record looks like. It did not
check `log_command`.

## Definition of done

- [ ] `grep -n 'shell=s1\|shell id ·' README.md` prints nothing. Today it
      prints 2 lines.
- [ ] The three lines (the two sample records and the `Fields for command
      records:` line) read exactly as they did before commit `4593e8ae`:
      `git diff 4593e8ae~1 -- README.md | grep -c 'mode=background pane=-'`
      prints `0`.
- [ ] `cargo test` reports `passed=2309 failed=0`, unchanged.
