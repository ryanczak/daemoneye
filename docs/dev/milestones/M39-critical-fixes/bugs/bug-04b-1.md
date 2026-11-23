# Bug 1 on phase-04b: the doctor counts an unreadable copy as matching its source

**Severity:** minor
**Status:** closed (fixed by `3cf4cb35`, verified at review 2026-11-23)
**Filed:** 2026-11-23

## What's wrong

`copy_drift` correctly returns `CopyDrift::Unknown` when either file cannot
be read. The architect checked this with a throwaway test: a mode-000
source gives `Some(Unknown)`. But `sudo_scripts_check` (`src/cli/doctor.rs`)
keeps only `Stale` and `Orphaned` copies for its warning. An `Unknown`
copy is dropped from the warning, and it still counts toward the `Ok`
detail, `"{n} installed sudo copy(ies) match their source"`. So a copy the
doctor could not compare is reported as matching.

The phase's constraint says a read error counts as "cannot tell", and that
it must be **reported**. No test covers an `Unknown` entry.

A smaller wording problem: with no copies installed, the row reads
`0 installed sudo copy(ies) match their source`.

The live probe on pinky confirmed the main flow: `Ok` while current,
`Warn` naming the script and `install-sudoers` after an edit, exit code 0,
and `source removed` after deletion.

## What should happen

- An `Unknown` copy is reported. The row is `Warn`, and the detail names
  the copy as unreadable or "cannot compare". An `Unknown` copy is never
  counted as matching. The exit code stays 0.
- The `Ok` detail says only what is true. With no copies, it says none are
  installed. With copies, it gives the number that match.

## Root cause

The `Unknown` variant was added to `CopyDrift` for read errors, but
`sudo_scripts_check`'s filter and its `Ok` count were written against
`Stale` and `Orphaned` only.

## Definition of done

- [ ] A new test, `sudo_scripts_check_reports_unreadable_copies`, passes.
      With one `Unknown` entry, the row is `Warn`, the detail names the copy
      and does not say it matches, and `exit_code` is 0. **The test does
      not exist on the current tree. Today the same input gives `Ok`.**
- [ ] `sudo_scripts_check_ok_when_none_or_current` also asserts that the
      no-copies detail does not contain `0 installed`.
- [ ] Mutation check, with the output pasted: drop `Unknown` from the
      warning filter again, see the new test fail, then restore it.
- [ ] All four gates pass, and the lib suite reports **at least 3086**
      passed.
