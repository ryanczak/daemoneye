# Bug 1 on phase-03a: `wait_for` with `settle_ms` never settles

**Severity:** major
**Status:** fixed (c2a34c39)
**Filed:** 2026-10-15

## What's wrong

`wait_screen` (`src/daemon/executor/knowledge/shell.rs`) runs the pattern
loop first and, on a match, **returns** `WaitOutcome::Matched` from the
function. When `settle` is also `Some`, the settle loop below is never
reached, so `read_shell_screen(wait_for: "…", settle_ms: 1500)` returns the
moment the pattern first appears — mid-draw — instead of after the screen
goes quiet. The settle loop's own `matched(&second)` check shows the combined
case was intended; the early return makes it dead.

## What should happen

Phase 03a Spec Task 3: "With both: first the pattern, then the settle. One
`timeout` bounds the whole wait." The combined call is the main Claude Code
use: wait for the answer's marker, then for the drawing to stop. With both
set, a match must hand over to the settle phase, and the outcome is
`Settled` (or `NotSettled` at the timeout), never `Matched`.

## Root cause

The `if wait_for.is_some()` loop in `wait_screen` returns
`WaitOutcome::Matched` unconditionally on a match, without checking whether
`settle` is set.

## Definition of done

- [ ] `wait_screen_with_both_waits_for_the_pattern_then_settles` exists and
      passes. The reader yields `"loading"`, `"done 1"`, `"done 2"`, then
      `"done 3"` from then on. With `wait_for` `done` and a settle ⇒
      `Settled`, **not** `Matched`, and the reader was called past the first
      `done` read.
- [ ] With `settle` `None`, `wait_for` alone still returns `Matched` (the
      existing `wait_screen_returns_when_the_pattern_appears` is unchanged
      and passes).
- [ ] `cargo test` ⇒ `passed=2385 failed=0`.
