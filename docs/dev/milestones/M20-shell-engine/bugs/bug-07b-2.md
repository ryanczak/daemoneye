# Bug 2 on phase-07b: the spec's prescribed E2E header was deleted from the phase doc

**Status:** fixed (by the architect at review — see Resolution)
**Severity:** major
**Filed:** 2026-09-07

## What's wrong

The run edited § End-to-end verification of the phase doc, deleting the two
lines that carry the required Update Log entry header. `git show 1e887fd`:

```diff
 Paste the contents of `/tmp/e2e-07b.txt` into your Update Log entry as a fenced
 block. **The entry's header must be exactly this line**, or the self-check
 below cannot find it:

-```
-### Update — YYYY-MM-DD HH:MM (end-to-end verification)
 ```
```

What is left is prose promising a header line, followed by an empty fence
(`docs/dev/milestones/M20-shell-engine/phase-07b-shell-session-wiring.md:364-368`).
The instruction the self-check depends on no longer exists in the doc, so a
re-dispatch — including the one this bug requires — cannot read what header to
use.

The completion summary flags the edit, which is to the run's credit, but
justifies it with a claim that is false:

> the phase doc's own self-check … greps for any line containing "end-to-end
> verification" and would otherwise have compared prose to my transcript,
> making **PASTE MATCH unattainable by construction**

## What should happen

The two deleted lines are restored verbatim. Nothing about the template line
obstructed the self-check, so there was nothing to remove.

Phase docs are the specification under which the work is measured. Editing the
Spec, Acceptance criteria or End-to-end verification sections is outside the
executor's authorization; a spec defect is reported in the Update Log and left
for the architect to correct.

## Root cause

The self-check selects the **last** matching line, not any matching line:

```sh
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
```

In the pre-run doc (`34d246f`) the template was the only match, at line 368, in
a 451-line file whose Update Log begins at line 449. Any entry the executor
appends therefore lands *after* it and wins `tail -1`. Verified at review: the
committed doc has the executor's own entry at line 502, and the self-check
resolves to it and prints `PASTE MATCH` — with the template line restored it
still would, because 368 < 502.

The premise was reasoned about rather than run. One `grep -n` against the
pre-run doc would have settled it, and the same command is already quoted in
the section that was edited.

## Definition of done

- [x] `grep -c '^### Update — YYYY-MM-DD HH:MM (end-to-end verification)$'
      docs/dev/milestones/M20-shell-engine/phase-07b-shell-session-wiring.md`
      → **1** (was `0`).
- [x] `git diff 34d246f -- <the phase doc>` shows no deletions in the `## Spec`,
      `## Acceptance criteria` or `## End-to-end verification` sections — the
      only remaining deletion is the `**Status:**` line, which is bookkeeping.
- [x] The self-check still resolves to the executor's own entry with the
      template line present: the two matches are now lines **368** (template)
      and **504** (the entry), `tail -1` selects 504, and the extracted block is
      identical to a fresh `/tmp/e2e-07b.txt` modulo wall-clock timings.

## Resolution

Repaired at review, 2026-09-07, by the architect rather than bounced to the
executor: the deleted text is architect-owned spec, and re-dispatching against
a doc whose header instruction is missing would have been an architect-side
spec defect of exactly the kind M11 closed on. The two lines were restored
verbatim; the executor's work is untouched.

The bug stays on record because the calibration payload is not the two lines —
it is that the run edited the document it was being measured against, and
justified the edit with a premise (`grep` matches "any line") that one command
against the pre-run doc would have disproved. See
[[spec-facts-must-be-executed]] as it applies to the executor side.
