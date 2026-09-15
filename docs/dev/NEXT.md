# NEXT

The one live pointer and nothing else. The append-only record of every earlier
milestone and phase — and every `**Superseded — …**` pointer that once stood
here — lives in [NEXT-HISTORY.md](NEXT-HISTORY.md).

## M22 — Tool surface & context (active, scoped 2026-09-12)

**Active phase: none.** Phase-06b — shell-history closed
**approved_first_try 2026-09-15**
(`docs/dev/milestones/M22-tool-surface/phase-06b-shell-history.md`, status:
done). Next up is **07 — run-terminal-command-reshape**, not yet drafted; run
`/rexymcp:architect next` to draft it.

`list_shell_logs { since?, limit? }` and `read_shell_log { log, command_index?,
lines?, filter? }` ship as the deferred **`history`** group — **48 tools: 34
core + 14 deferred**. The group needed no machinery: `tools_in_group` derives
it from `TOOLS`, and a criterion confirmed **no hard-coded group list was
added anywhere**. The three design parameters that do not exist on this tree
(`host`, `owner`, `offset`) stayed out.

**06b also closed the retention defect 06a made live** — the sweep now
accepts `meta_path_for`'s spelling, and its fixture is built through the
producer so writer and test cannot drift apart again.

**First clean run of the milestone's back half: `approved_first_try`, 448
turns, no bounce, no escalation.** After 06a's three runs and a takeover, the
difference was a phase deliberately kept host-free — all seven tests are
directory-driven — and out of `log.rs`'s raw-bytes-versus-decoded-payload
seam, which a criterion asserted was untouched.

**Calibration, and it is the same lesson twice now.** Both of my first-attempt
mutants at review were **no-ops against the code as written**: one disabled a
shape gate that merely fell through to another error the test tolerates, and
one `continue`d in an enrichment loop that runs *after* every row is already
pushed — the implementation scans in two passes, so the spec's single-pass
mutation is unexpressible against it. Re-constructed against the actual
structure, both reddened **6 of 6**. With 06a's withdrawn mutation this is the
second phase running where **the mutation was the proxy, not the test**.

One more architect instrument error, corrected in place: a criterion demanded
`grep -c 'fn read_shell_log' → 0` in the very phase that *adds*
`fn read_shell_log`. The executor flagged it, held the intent, and did not
contort the code to satisfy a literal — the right call. Eight instrument
errors across M22, and this is the third of the sub-species *the criterion
forbids the thing the phase creates*.

**Next step:** `/rexymcp:architect next` drafts **phase-07 —
run-terminal-command-reshape**. Phases 07–10 are not drafted; each is drafted
after its predecessor is approved.

**Owed at milestone close, now at three occurrences — fold it.** Six
architect-side instrument errors across M22, and the deeper version has hit
three times: a criterion pinning a test's **name** where its *discriminating
power* was what mattered (04b's `bug-04b-2`, 05's `bug-05-1`, and the
`shell::` substring conflation). `WORKFLOW.md` § "Specs pin behavior, not
rendering" covers the class and is not being followed. Proposed fold: **for
any test whose job is to pin a safety or negative property, the phase doc
must name the mutation that makes it red — not just the test's name.**

**Carried out of M24, still open:**

- One calibration fold **held at two occurrences**: *state the property to
  break, not the line to edit* (M24 09c, 10). M22 has nine near-identical tool
  phases; a third instance here should be folded.
- **A phase doc is append-only and nothing enforces it** — M24 phase-10's run
  overwrote its own spec, and no gate caught it. Wants a guard test or a
  template line; until then the reviewer checks the headings survived.
- One local-only `WORKFLOW.md` edit on the upstream push backlog: the
  `NEXT-HISTORY.md` row in the directory layout.
- **Deferred to M25 with the governor:** cross-run telemetry (`RunRecord` →
  scorecard/profile).

## Historical

### M24 — Run Manager: **closed 2026-09-12, PE signed off**

**Active phase: none.**

All **16** M24 phases are `done` as of 2026-09-12 (10 charter rows; 05, 06, 07
and 09 split at drafting on measured seams). The milestone retrospective is in
`docs/dev/milestones/M24-run-manager/README.md` § Retrospective. **PE signed
off 2026-09-12.** Carried out of the close, still open:

1. **A phase doc is append-only, and nothing enforces it.** Phase-10's run
   overwrote its own spec (556 lines, including the `Status:` line), and no gate
   caught it — the suite was green with the spec gone and `bug_tracker` passed
   with no status line. Restored from git at review. Wants a guard test or a
   line in the phase template.
2. **Calibration folds — PE decided 2026-09-12.** Two of the three proposals
   turned out to already exist in `WORKFLOW.md` and had not been followed (the
   `## Spec` mutation-task rule, 2026-08-08; corpus scoping, 2026-08-29).
   - **Landed:** the after-the-phase corpus rule, as a fourth row on the
     existing scoping table plus one paragraph (14 instances, all
     architect-side).
   - **Landed:** *pin evidence where it lives*, as a clause on bounce step 3
     (07b ×2).
   - **Held at 2:** *state the property to break, not the line* (09c, 10). It
     tenses against the existing "exact `old_str`" prescription; resolve on a
     third occurrence.
3. **One local-only `WORKFLOW.md` edit is on the upstream push backlog**: the
   `NEXT-HISTORY.md` row in the directory layout.
4. **Deferred to M25 with the governor:** cross-run telemetry (`RunRecord` →
   scorecard/profile).

**Next step:** `/rexymcp:architect` scopes the next milestone. Nothing is dispatched until
then.

### M24 — Run Manager: the phase pointers

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
