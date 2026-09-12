# NEXT

The one live pointer and nothing else. The append-only record of every earlier
milestone and phase — and every `**Superseded — …**` pointer that once stood
here — lives in [NEXT-HISTORY.md](NEXT-HISTORY.md).

## M24 — Run Manager (active)

**Active phase: phase-09a — resume-seed**
(`docs/dev/milestones/M24-run-manager/phase-09a-resume-seed.md`, status: todo,
drafted 2026-09-12). Dispatch with `/rexymcp:dispatch phase-09a`.

Phase-08 closed **approved_first_try 2026-09-12** (`5138a33`). Three of its
acceptance criteria were wrong as drafted — all mine — and the executor caught
all three unprompted, explained each structurally and re-verified with correct
greps. The rule that generalises them: **validate a structural criterion against
the tree as it will look after the phase, not only before.**

Scope for 09a: the resume **seed**, pure. Given a finished run's id and a
directive, read its turn log, take the kind from the first `RunStarted` and the
briefing from the last `RunFinished`, and render the text a resumed run starts
from. `Complete` and `Cancelled` runs carry no briefing and are still resumable
on the directive alone; a run with no `RunFinished` is **not** resumable. 10
tests, nine of them pure.

The design's "not transcript rehydration" clause becomes structural:
`render_seed(kind, Option<&Briefing>, directive)` cannot see `TurnLogRecord`,
and a criterion pins that.

Pre-injected trap: **two unrelated types are called "briefing"** —
`daemon::briefing::read_briefing(agent)` (G4 prose) versus `run::Briefing`
(phase-01's bounded struct). 09a uses only the second, and a criterion asserts
the first is not referenced from `src/daemon/run/` at all.

Phase 09 split three ways: **09a** seed (this), **09b** wiring (CLI + IPC +
re-spawn), **09c** seeded tasks from a runbook `## Steps` section — none of
which exists in the tree today. Then 10 (cli). 09b and 09c are not drafted.

**Superseded — M24 phase-08 — scheduled-run**
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
