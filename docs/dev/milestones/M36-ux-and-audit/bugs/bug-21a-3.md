# Bug 3 on phase-21a: the sbx pass-through test can't fail

**Severity:** minor
**Status:** fixed (e0b1dccd)
**Filed:** 2026-11-09

## What's wrong

`sbx_status_passes_programs_through` (`src/shell/sbx_backend.rs`) asserts
only that `programs` is empty, on an inner bash that holds no records. A
`SbxBackend::status` that hard-codes `programs: Vec::new()` passes it. So
Behaviour 5's pass-through is untested.

## What should happen

The inner backend holds at least one record before the outer `status` is
read: a `done` report fed to the inner shell (`done` survives the program's
exit). The outer frame then carries that record unchanged.

## Definition of done

- [ ] `sbx_status_passes_programs_through` asserts that a non-empty record
      list equals what the inner backend's own `status` reports.
- [ ] Replacing the pass-through with `Vec::new()` makes the test fail.
      Paste that mutation run into the Update Log.
