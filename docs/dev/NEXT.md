# NEXT

The one live pointer and nothing else. The append-only record of every earlier
milestone and phase — and every `**Superseded — …**` pointer that once stood
here — lives in [NEXT-HISTORY.md](NEXT-HISTORY.md).

## M24 — Run Manager (active)

**Active phase: phase-10 — run-listing**
(`docs/dev/milestones/M24-run-manager/phase-10-run-listing.md`, status: todo,
drafted 2026-09-12). Dispatch with `/rexymcp:dispatch phase-10`.
**This is M24's last phase** — when it is `done`, the milestone hits its
boundary, which is a human sign-off gate.

Phase-09c closed **approved_first_try 2026-09-12** (`bb5eb80`), closing M24's
last *behavioural* exit criterion. The mutation-checks-as-Spec-task fold worked
first time, and the executor caught a spec defect from the inside: mutation 2
told it to drop a range check its implementation does not have (out-of-range
indices are excluded structurally), so it reported the vacuous green and found
the mutation that does discriminate.

Scope for 10: the **discovery** surface. Nine phases have written
`var/log/runs/<id>.jsonl` and **nothing reads that directory** — every command
so far takes an id the user had to find by hand. `run/list.rs` adds
`RunSummary`, a pure `summarize`, a `list_runs()` that enumerates the dir, and a
pure `render_runs_table`; `daemoneye run list` and the `/runs`, `/run <id>`,
`/stop <id>` slash commands are the surfaces. All read `$HOME` directly — no
daemon round trip, pinned by a criterion that none calls `request_once`.

Naming: the exit clause says `daemoneye runs`, but 09b established the `Run`
group, so the command is `daemoneye run list`. Substance met, wording deviation
recorded.

`/run` and `/stop` are ten-line wrappers over `run_log_tail` and `request_stop`,
which already exist — criteria pin that the slash module re-implements neither.

Deferred to M25 with the governor: cross-run telemetry (`RunRecord` →
scorecard/profile), which the milestone README always listed as a candidate.

**Superseded — M24 phase-09c — seeded-tasks**
(`docs/dev/milestones/M24-run-manager/phase-09c-seeded-tasks.md`, status: todo,
drafted 2026-09-12). Dispatch with `/rexymcp:dispatch phase-09c`.

Phase-09b closed **escalated 2026-09-12** (`67028e9`) after a `hard_fail` on one
invented API and a resume. Its `## Mutation checks` went unrun — the instructions
sat outside `## Spec` and were never seeded as tasks — so the architect ran both
at review; both go red. **09c makes the mutation checks Spec task 6**, which is
the fix for that class.

Scope for 09c: M24's last behavioural exit criterion. `parse_steps` reads a
runbook's `## Steps` section; the ghost's system prompt gains a marker contract;
and the completion break at `src/daemon/ghost.rs:1203` is gated on
`uncovered_steps` — the ghost may not finish until every seeded step has a
literal `STEP <n>: DONE` or `STEP <n>: SKIPPED <reason>` line.

The emphasised half of the criterion — *a condition it can check, not an
instruction it can agree with* — is why coverage is a parsed marker rather than
a keyword scan. The codebase already learned this: `parse_ghost_trigger`'s doc
comment records that the loose `ALERT` heuristic was removed "because a model
remarking 'no ALERT condition' could spuriously launch a ghost". A transcript
saying "I could not complete step 2" must leave step 2 **uncovered**, and that
is the load-bearing negative test.

Deliberate limits: only the one completion break is gated (cancel, wrap-up and
session-gone stay ungated, pinned by a criterion); the existing turn budget is
what stops a ghost that refuses to mark its steps, so no new terminal state and
no second counter; and no `complete_step` AI tool — a parsed line is already a
checkable condition, and a tool would drag in `TOOLS`, three provider renderings
and the doc_truth tables.

After 09c only **10** (cli listing) remains, and it is not drafted.

**Superseded — M24 phase-09b — resume-wiring**
(`docs/dev/milestones/M24-run-manager/phase-09b-resume-wiring.md`, status: todo,
drafted 2026-09-12). Dispatch with `/rexymcp:dispatch phase-09b`.

Phase-09a closed **approved_first_try 2026-09-12** (`fe3f6f3`). Its executor
recorded a vacuously-green mutation placement before finding the one that
discriminates, and disclosed an intermediate `PASTE MISMATCH` before correcting
it — both reproduced at review. That is the standard the milestone's two earlier
bounces were asking for.

Scope for 09b: the four surfaces that make `daemoneye run resume <id>
"<directive>"` real — a pure `resumable_runbook` rule, `Request::ResumeRun`,
a daemon handler that re-spawns the ghost seeded with 09a's text, and a `Run`
CLI subcommand group. **The seed is the alert message** passed to
`GhostManager::start_session_with_config`; that is the whole point.

09b also **retires `resume_seed`** in favour of `resume_plan`. 09a shipped it
with zero production callers, and `resume_plan` supersedes it by resolving the
runbook too — STANDARDS § 2.2 says delete an unused symbol rather than keep a
second entry point speculatively.

Only a **ghost** run is resumable: it is the only kind carrying a runbook name.
Chat, scheduled and delegated are refused with a reason the user reads, each a
named negative test — `Delegated` being the subtle one, autonomous but naming an
agent rather than a runbook.

Pre-injected: `trigger_ghost_turn` must be `tokio::spawn`ed **after** the IPC
reply, never awaited inside `handle_client` — it runs a whole AI loop while the
client waits.

Remaining after 09b: **09c** (seeded tasks from a runbook `## Steps` section)
and **10** (cli listing). Neither drafted.

**Superseded — M24 phase-09a — resume-seed**
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
