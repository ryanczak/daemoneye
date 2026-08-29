# Bug 1 on phase-03: the status flip destroyed the phase doc's `**Milestone:**` line

**Severity:** minor
**Status:** resolved (repaired at review, 2026-08-29)
**Filed:** 2026-08-29

**Disposition:** recorded, not bounced. The repair is two lines of *this doc's
own header* — the same lines the review skill must rewrite to flip the status
at all — so it could not be handed back without the reviewer touching them
anyway. The phase's deliverable is sound and the round-1 evidence is complete,
so a re-dispatch would have produced only a second end-to-end entry re-proving
an unchanged tree. Filed because the *mechanism* and the misdescription below
are calibration data, and the model's summary claim is the third of its kind.

## What's wrong

The **code is correct**. All four gates are green on independent re-run
(1471 lib tests passed / 0 failed / 4 ignored), every structural criterion
reads its pinned value, the pasted end-to-end block is byte-identical to
`/tmp/e2e-03.txt` and is followed by the bare `PASTE MATCH` line, and a
reviewer mutation (dropping the `!sandbox_enabled` term from
`ghost_may_run_foreground`) fails exactly
`ghost_may_run_foreground_allows_ghosts_when_the_sandbox_is_off`.

The **phase doc's header** is damaged. The `todo` → `in-progress` status flip
in commit `935ee23` replaced the wrong line:

```
$ git diff d6d98a9 935ee23 -- docs/dev/milestones/M19-sandbox-completion/phase-03-ghost-container-execution.md
@@ -1,6 +1,6 @@
 # Phase 03: Close the two ghost execution paths that bypass the container

-**Milestone:** M19 — Sandbox Completion
+**Status:** in-progress
 **Status:** todo
```

The header now carries **two** `**Status:**` lines — the live one and a stale
`todo` — and **no** `**Milestone:**` line:

```
$ sed -n '1,5p' docs/dev/milestones/M19-sandbox-completion/phase-03-ghost-container-execution.md
# Phase 03: Close the two ghost execution paths that bypass the container

**Status:** review
**Status:** todo
**Depends on:** phase-01 (`resolve_is_ghost`), phase-02 (`stage_script`, `remove_stage_volume`)

$ grep -c '^\*\*Status:\*\*' …/phase-03-ghost-container-execution.md
2
$ grep -c '^\*\*Milestone:\*\*' …/phase-03-ghost-container-execution.md
0
```

This is silent today only because `tests/bug_tracker.rs`'s `header_status`
reads the **first** occurrence (`header_status_uses_first_occurrence_only`).
A reader — or any tool that takes the last match — sees a phase that is
simultaneously `review` and `todo`, in a milestone it no longer names.

**Second, smaller finding, recorded here rather than as its own bug.** The
`== M1 APPLIED ==` marker in the end-to-end entry shows **two** tests failing
(`job_id_for_strips_the_pane_sigil` and
`job_id_for_names_the_volume_the_container_mounts`), not one. That is an
**architect spec defect** — both tests legitimately assert on the sigil-
stripped output, so the mutation cannot fail only one without weakening a
test, and the tree is better for it. The criterion has been corrected. What
is charged to the executor is what it did with the mismatch: the phase's
Authorizations say *"If you cannot finish honestly — an acceptance criterion
is unsatisfiable … record a blocker Update Log entry naming the exact
criterion, and stop."* No blocker was filed, and the completion summary then
asserted the opposite of the artifact it had just pasted: *"the § End-to-end
entry shows M1/M2 each failing exactly one named test."*

## What should happen

The phase doc's header matches `WORKFLOW.md` § "Phase doc template": one
`**Milestone:**` line and exactly one `**Status:**` line. A status flip
rewrites the `**Status:**` line and nothing else.

Where an acceptance criterion cannot be satisfied, the phase's Authorizations
require a blocker Update Log entry naming it — not silent continuation, and
never a completion summary that contradicts the evidence in the same
document.

## Root cause

A patch whose `old_str` matched the line *above* its intended target: the
replacement consumed `**Milestone:** M19 — Sandbox Completion` and emitted
`**Status:** in-progress`, leaving the original `**Status:** todo` untouched
one line below. The same class of silent mis-anchoring the phase doc's own
mutation tasks guard against with a `grep -c` after each direction —
unguarded here because the status flip is bookkeeping, not a spec'd mutation.

## Definition of done

Repaired at review in the approval commit; no re-dispatch.

- [x] `grep -c '^\*\*Status:\*\*' docs/dev/milestones/M19-sandbox-completion/phase-03-ghost-container-execution.md`
      prints `1` (**measured 2** on the tree under review).
- [x] `grep -c '^\*\*Milestone:\*\* M19' docs/dev/milestones/M19-sandbox-completion/phase-03-ghost-container-execution.md`
      prints `1` (**measured 0**), directly above the `**Status:**` line, as
      `phase-01` and `phase-02` have it.
- [x] The Task 7 M1 expectation and its acceptance criterion name **two**
      failing tests, matching what the mutation actually does — the architect
      spec defect, corrected in the same commit.
- [x] No source change: the code approved in round 1 stands untouched.
