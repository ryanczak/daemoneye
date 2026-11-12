# Bug 1 on phase-21c: state reports never reach the terminal

**Severity:** blocker
**Status:** fixed (phase-21c re-dispatch, 2026-11-12)
**Filed:** 2026-11-12

## What's wrong

`Emitter::report` (`src/cli/program_status_emit.rs:237`) appends every
`working`, `blocked` and `idle` report to the in-memory `inner.out` buffer
(line 247) and nowhere else. The computed bytes are then discarded
(`let _ = bytes;`, line 253). Only `Emitter::write` (line 258) copies bytes
to stdout, and only `finish` calls it, while the exit hook `exit_clear`
writes `clear` itself.

So in production the terminal gets a `clear` at exit and **no state report
ever**. The milestone's exit criterion fails: a 7501-aware terminal never
sees the client as `blocked`. The buffer also grows for the life of a
`chat` session.

The tests pass because every one reads `Emitter::written()`, the same
buffer. Nothing checks that a report reaches the writer production uses.

## What should happen

Behaviour 2–6 and the Constraint: "It writes through an injected writer
(stdout for real, a buffer in tests)." Each report goes to the emitter's
one writer. `for_stdout` gives it stdout. A test gives it a buffer it can
read back through its own handle, not through an emitter-private copy. No
production-path output depends on `cfg!(test)`.

## Root cause

The writer was never injected. The emitter owns a private `Vec` and uses a
compile-time flag (`TO_STDOUT`) as a stand-in for the writer, and `report`
skips even that flag.

## Definition of done

- [ ] `program_status_emit_reports_reach_the_injected_writer` passes. It
      builds an emitter over a test-held writer, sends `working`, `blocked`
      and `idle`, and reads all three reports back from the writer the
      test holds, not through `written()`. **Today no constructor accepts a
      writer, so the test can't be written against the current tree.**
- [ ] `TO_STDOUT` and the `cfg!(test)` branch are gone:
      `grep -c 'cfg!(not(test))' src/cli/program_status_emit.rs` prints 0.
      Today 1.
- [ ] Every existing `program_status_emit_` test still passes.
