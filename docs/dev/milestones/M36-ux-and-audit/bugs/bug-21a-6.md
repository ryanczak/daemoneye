# Bug 6 on phase-21a: a re-report keeps its first stamp, so a program's second run loses its working record

**Severity:** major
**Status:** open
**Filed:** 2026-11-10

## What's wrong

`Stamps::stamp_missing` (`src/shell/program_status.rs`) writes an id's epoch
only when the ledger has no entry for that id. The ledger is pruned to the
ids the table still holds, and an expired record's entry isn't removed at
all. So a later report for an id the ledger already knows keeps the
**first** report's epoch. The doc comment says so: "never backwards: a
re-report by a later arrival must not rewrite an earlier stamp".

Here is how a program fails on its second run:

1. Line 1: `tool` reports `working` and then `done` on the root record. The
   record is `done`, stamped with epoch N.
2. Line 2, entered at the prompt, makes the epoch N+1. `tool` reports
   `working` again and keeps running.
3. A read with a job in front sees stamp N < N+1 and drops the record.

The reviewer reproduced this with a throwaway test. Line 1 ran
`/bin/printf '\033]7501;state=working\007'; /bin/printf '\033]7501;state=done\007'`.
After the shell went idle, line 2 ran
`/bin/printf '\033]7501;state=working\007'; sleep 30`. A `status` 1.5 s
into the sleep returned `programs: []`.

The same happens when a `working` record expires at a read and the same id
reports again later. The leftover ledger entry pins the old epoch.

## What should happen

A report's epoch is the epoch **that report** arrived in. Every report,
new or replacing, records the current epoch for its id. The lifetime
rules from bug-21a-5 stay exactly as they are, and so do all the tests
that pin them.

## Root cause

The ledger was designed for bug-21a-2's foreground-group stamps. There,
"don't rewrite an earlier stamp" guarded against a race over who printed a
report. Under the input-line epoch that guard is wrong: a later report is
always the newer fact about its id.

## Definition of done

- [ ] `backend_status_keeps_a_working_report_when_its_program_runs_again`
      passes. It is the reproduction above, written as a test: same id
      (root), line 1 ends `done`, line 2 reports `working` and sleeps, and a
      `status` during the sleep carries the `working` record. **This test
      doesn't exist yet. The reviewer's equivalent probe fails on the
      current tree.**
- [ ] `backend_status_keeps_a_working_report_after_its_record_expired_and_it_reports_again`
      passes. A `working` record is dropped at a read when its line ends.
      The same id then reports `working` in a new line whose program
      sleeps. A `status` during the sleep carries it.
- [ ] Every test named in the phase doc still passes.
