# NEXT

**Active phase: none.**

**M10 — Residual Hygiene closed 2026-08-02** (three phases, all
`approved_first_try`, zero bugs, zero bounces). Retrospective:
`docs/dev/milestones/M10-residual-hygiene/README.md`.

M7, M8, M9 and M10 are all closed and **no milestone is scoped**. Starting one is
a human decision — the architect does not cross a milestone boundary on its own.

## The carried list is empty except for one unreproducible item

1. **`hooks_land_on_private_server`** — the old phase-04-review flake. Binds no
   ports; **0 failures in 300 runs** across M8, M9 and M10. No evidence to work
   from. Only a bug if it recurs.

Everything else carried out of M7 and M8 is closed: the tty tests fail instead of
hanging, the memory category→directory mapping is derived in all three callers,
the last real-clock sleep is gone, and `daemoneye reindex` is documented and gated.

## One decision waiting on the PE

The executor has now mislabelled its own model in its Update Log entry **three
times** (M9 phase-01 "Claude (sonnet-4.5)", M10 phase-01 "Claude executor", M10
phase-03 "Claude (claude-opus-4-5-20251101)"). It is Qwen3.6-27B-FP8 every time;
each was corrected at review, and the server-authored tail always records the
correct model, so telemetry is unaffected.

Three occurrences is the fold threshold in `WORKFLOW.md` § Calibration. Folding
requires PE sign-off, so nothing has been changed. The options are roughly: state
in the executor contract that the Update Log must carry the configured model
name; drop the model line from the executor's own entry and rely on the
server-authored tail; or accept it as cosmetic and stop correcting it.

## The rules M7–M10 earned

> **Do not assert a fact about the system in a spec unless it was executed.**
> A *claimed failure mode* is such a fact — M9 justified a test with a
> compile-time impossibility one `cargo build` would have disproven.
>
> **A criterion about the tree the phase will produce must be validated against
> that tree**, not the one in front of you. Calibrating against the current tree
> catches unsatisfiable criteria; it does not catch criteria the phase's own work
> invalidates, or criteria that already pass without the work being done.
>
> **Prototype the change and mutate it before writing the spec.** M10 phase 02's
> real risk — two labels with no test, where a swap left 1036 tests green — was
> invisible until the prototype was mutated.
>
> **An acceptance criterion for an intermittent failure must be a repeat count
> derived from a measured rate.** A single green run is not evidence.
>
> **Measure through the same door the user will use.** M9's in-process probe of
> `reconcile_index()` recorded a bare-`$HOME` result the shipped binary never
> produces.

Corollaries, each earned more than once: naming a false-success mode is worthless
unless the guard is checked against it; a phase that lands code for a *later*
phase must say how the deny-warnings gate is satisfied; and **a green bounce
always needs a refined re-dispatch**, never a plain one.
