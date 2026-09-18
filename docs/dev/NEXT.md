# NEXT

**Active phase:** none in progress — `phase-10b-prompt-assets-seeded` was
approved 2026-09-18 (`approved_first_try`).

**Every M22 row is now `done`** (01, 02, 03a–c, 04a–b, 05, 06a–b, 07a–b,
08a–b, 09a–d, 10a–b). The milestone is **not yet closed**: the retrospective,
the calibration folds and the final `NEXT.md` state belong to milestone close.

Run `/rexymcp:architect` to close M22 — write the retrospective in the
milestone README, decide the calibration folds, and set the active phase.

**Two calibration items are queued for that decision**, both recorded in the
10b review verdict:

1. **Pinned counts that the phase's own artifacts make unreachable** — 08b and
   10b both shipped a criterion that could not hold because the Spec required
   an artifact containing the very string being counted. Two occurrences; a
   third makes it a fold.
2. **The PASTE MATCH self-check verdict line went missing from 10b's
   end-to-end entry** — third occurrence of a class `WORKFLOW.md` already
   folded, which means the fold is not self-enforcing. Proposed structural
   fix: make the self-check verdict a numbered acceptance criterion rather
   than Task prose.
