# NEXT

The one live pointer and nothing else. The append-only record of every earlier
milestone and phase — and every `**Superseded — …**` pointer that once stood
here — lives in [NEXT-HISTORY.md](NEXT-HISTORY.md).

## M24 — Run Manager (active)

**Active phase: phase-08 — scheduled-run**
(`docs/dev/milestones/M24-run-manager/phase-08-scheduled-run.md`, status: todo,
drafted 2026-09-11). Dispatch with `/rexymcp:dispatch phase-08`.

Phase-07b closed **approved_after_2 2026-09-11** (`99c6e29`). Both bounces were
evidence defects on correct code — the mutation-check-2 transcript, first in
prose (bug-07b-1) then surviving in the fenced block (bug-07b-2); the shipped
code never changed across either. Calibration recorded in the phase doc and the
milestone README: **pin evidence where it lives**, and scope doc-content
criteria to the section they police.

Scope for 08: `RunKind::Scheduled` gets its **first production constructor** —
it has existed since phase-01 and nothing has ever built one. `run_scheduled_job`
(435 lines, seven returns, eleven `mark_done` sites, returns `()`) is renamed to
`_inner` and wrapped; the wrapper holds a `RunLogGuard` and derives the terminal
`RunState` from the status the store holds after the job ran. Pure mapping
function, 7 tests — `daemon::scheduled` has **no test module at all** today.

Pre-injected gotcha: a successful **`Every`** job reads back as `Pending`, not
`Succeeded` (`mark_done` advances and reschedules it), so the `Pending` arm must
map to `Complete`. Pinned as a named test and as mutation check 1.

Out of scope with reasons stated: scheduled-job cancellation (the script branch
lives in a tmux window the substrate replaces; the ghost branch already cancels
via 07b), collapsing the two run logs a scheduled ghost job produces, and
rewriting the eleven `mark_done` sites.

Remaining after 08: 09 (resume), 10 (cli). Neither drafted.
