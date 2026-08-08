# Development Workflow

How a project is built under architect-driven development: who does what, what a
phase looks like, and how work moves from "planned" to "merged."

## Roles

**Principal engineer / architect.** Owns the architecture, breaks design into
phases, reviews completed phases, writes bug reports, decides scope changes. Does
not normally write implementation code — that's the executor's job.

**Executor.** Implements one phase at a time following the phase doc. Reads
`STANDARDS.md` at the start of every phase. Reports blockers when stuck. Never
invents scope. Never edits files outside the phase's authorization.

**Human (project owner).** Decides direction, vetoes architectural choices, runs
the show. Both the architect and the executor work for the human.

---

## Hierarchy

```
Milestone           — a coherent capability (M1 Foundations, M2 Tools, ...)
└── Phase           — one executor session's worth of work; one markdown file
    └── Task        — a single concrete change (one function, one file, one test)
```

A **milestone** is large (weeks of work). A **phase** is small (one focused
executor session, ideally < 500 lines of diff). If a phase is bigger than one
session, it's two phases — re-split it.

---

## Directory Layout

 ```
 docs/dev/
 ├── NEXT.md                           pointer to the active phase; executor reads first
 ├── STANDARDS.md                       engineering contract; read every phase
 ├── WORKFLOW.md                        this file
 └── milestones/
     └── M<n>-<slug>/
         ├── README.md                  milestone overview
         ├── phase-01-<slug>.md         a phase doc
         ├── phase-02-<slug>.md
         └── bugs/
             └── bug-<phase>-<n>.md      review-finding bug reports
 ```

 `NEXT.md` is maintained by the architect and tells the executor which phase to
 work on next. At a milestone boundary it says "none", signaling the human gate.
 The executor reads it before every session to locate the active phase doc.

 Phases are numbered in execution order. Phases that can run in parallel share a
 parent number with letter suffix (`phase-03a-x.md`, `phase-03b-y.md`).

---

## Milestones

Milestones come from the project plan. Each entry becomes a milestone with its
own `M<n>-<slug>/` directory. The architect expands a milestone into phases
**on demand, not all at once**, because earlier phases reveal information that
shapes later ones.

### Milestone README template

```markdown
# M<n> — <Title>

**Goal:** <one sentence: what capability this milestone unlocks>

**Status:** planning | in-progress | review | done

**Depends on:** M<earlier> (or "none")

**Exit criteria:**
- <verifiable condition>
- <verifiable condition>

## Architecture references

- `docs/architecture.md#<section>`

## Phases

| #  | Phase                                  | Status      |
|----|----------------------------------------|-------------|
| 01 | <slug> ([phase-01-<slug>.md](...))     | todo        |
| 02 | <slug> ([phase-02-<slug>.md](...))     | todo        |

## Notes

<freeform: design decisions made during the milestone, dead ends, things
future milestones depend on>
```

---

## Phases

A phase is **one self-contained unit of implementation work** an executor can
complete in one session without ambiguity. Phase specs are written to leave no
scope or architecture decisions open; the executor picks implementation details
unless the spec is explicitly prescriptive.

The `Tags:` frontmatter line categorizes the phase (language, kind, size) so
metrics can be aggregated. The architect sets it when drafting; keep the
vocabulary consistent across phases.

### Phase doc template

```markdown
# Phase <n>: <Title>

**Milestone:** M<n> — <name>
**Status:** todo | in-progress | blocked | review | done
**Depends on:** phase-<m> (or "none")
**Estimated diff:** ~<n> lines
**Tags:** language=<rust|go|python|ts|...>, kind=<feature|refactor|bugfix|test>, size=<s|m|l>

## Goal

<One or two sentences. What does this phase accomplish? Why now?>

## Architecture references

Read before starting:

- `docs/architecture.md#<section>` — <one line on why>

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

<What exists in the repo today that this phase will modify. Specific file paths
and line numbers. Quote the relevant code if short.>

## Spec

Numbered tasks in execution order. Each names the exact file to edit and the
change to make. Three formats are accepted by the task seeder and all populate
the executor's Tasks panel:

- **List item:** `N. **<Task name>** — in \`<path>\`, <change>.` — concise;
  good when each task fits on one line.
- **Numbered subheading:** `### N. <Task name>` followed by detail paragraphs —
  good when a task needs code examples or sub-steps.
- **`Task`-prefixed subheading:** `### Task N — <Task name>` followed by detail
  paragraphs — the same as the numbered subheading, written in the natural
  "Task N" prose style. The separator after the number may be an em-dash
  (`—`, U+2014), a colon (`:`), or a dot (`.`).

All three can coexist in the same `## Spec` section. The seeder keys each task by
its number `N`, so the executor's `update_task(id="N", …)` calls match the seeded
ids. The section ends at the next `## ` heading (two hashes + space). A decimal
like `### 1.5x` is deliberately **not** seeded (it is not a task).

1. **<Task name>** — in `<path>`, <change>. <Why if non-obvious.>

## Acceptance criteria

Verifiable conditions — each one checkable by running a command or reading a file.

- [ ] `<command>` produces `<expected output>`.
- [ ] Test `<test_name>` passes.

## Test plan

Concrete tests to write — names + what they assert. Typically unit tests against
hermetic fakes (temp directory, mocked AI client, fixture replay).

- `test_<name>` in `<path>` — asserts <behavior>.

## End-to-end verification

Unit tests with hermetic fakes can pass while the real artifact the phase ships
is broken. For every acceptance criterion that references a real artifact (a
checked-in file, a CLI behavior, a binary entrypoint, a config the running binary
loads), verify against that real artifact before reporting complete, and quote
the actual output in the completion Update Log.

**Capture mechanically, never by hand.** Redirect each command's output to a file
and paste that file's contents. Do not retype a transcript, reconstruct one from
memory, summarise the results into prose, or copy lines out of the phase doc or a
previous Update Log entry — those are the four ways an untrue line gets into an
otherwise-real transcript. See § "A pasted transcript is a claim, not evidence."

**Put it in its own entry, and know what does not count.** The evidence goes in
an Update Log entry you author, titled `### Update — <date> (end-to-end
verification)`. **The server-authored `(complete)` entry never satisfies this.**
It carries a "Command output tails" block showing the gate commands' output —
which looks like captured evidence and is not: every phase gets it
automatically, and it proves the gates ran, not that the phase's acceptance
criteria were exercised against real artifacts. If the only new content in the
Update Log is that block, the requirement is unmet no matter how accurately the
completion summary describes what was run.

**One entry per dispatch, not per phase.** A phase that bounces and is
re-dispatched needs a **new** end-to-end entry for the round that changed the
code. An entry written in an earlier round does not carry forward: it describes a
tree that no longer exists, and the round whose diff is under review then has no
captured evidence at all. This applies to bug-doc Verification items too — if a
bug doc asks for a command to be run and its output pasted, that is this round's
entry, not a box the earlier round's entry already ticked.

Concretely: if the Update Log's newest content for the current dispatch is a
`(progress)` entry plus the server-authored `(complete)` entry, the requirement
is unmet, exactly as it would be on a first dispatch.

**For the architect: give the commands as a runnable block, never as prose.**
Write the exact shell the executor should run — redirect included, `exit=$?`
marker included — rather than describing what to verify. Where a result's success
case produces *no output* (a grep that finds nothing, a diff over identical
inputs), the exit marker is the whole proof; an empty block on its own
demonstrates nothing.

*(Folded 2026-07-31 after M6, on PE sign-off. Ten of that milestone's fourteen
bounces and two of its four architect takeovers were this single requirement —
more than every other cause combined. It was never a capability problem: in each
case the executor had run the commands and its narrative claims held up when
checked independently. Two things separated the runs that produced the artefact
from the ones that did not, consistently: whether the spec gave a literal
copy-pasteable block or prose, and whether it said in as many words that the
server-authored `(complete)` block does not count. Both were used inline in M6
phases 05 and 07 and worked immediately both times; omitting either failed in
phases 03, 04, 09, 11 and 12.)*

*(Per-dispatch clause folded 2026-08-04 after M11, on PE sign-off. Three
occurrences hit the threshold: phase-01 round 1, where the prose substitute was
measurably false — it cited a test filter that ran 3 of 6 new tests plus an
unrelated pre-existing one; phase-02a round 2, prose but true; and phase-02b
round 2, where no entry was written for the dispatch at all and a bug doc's
binary-level Verification item went unperformed. All three share one mechanism:
the phase already carried an entry from an earlier round, so it read as
compliant while the round under review had no evidence. The prior wording said
"an Update Log entry you author" without saying **when**, and could not
distinguish the two cases. Note the failure mode this closes is not a wrong
result — in the second and third cases the claims held up when checked
independently — it is that the check moved from the executor to whoever reviews,
silently.)*

If the phase ships **no** runtime-loadable real artifact (a pure internal
refactor, a new private type, a test-only helper), write:

> Not applicable — phase ships no runtime-loadable artifact. <one sentence why>

## Authorizations

If this phase needs anything from STANDARDS.md §5, declare it here:

- [ ] May add dependencies: `<dependency-name>`.
- [ ] May touch `docs/architecture.md` (specifically: <which section>).

(If nothing is authorized, write "None.")

## Out of scope

What the executor must **not** do, even if tempted. Things that look related but
belong to a later phase.

- <scope boundary>

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
```

---

## Update Log entries

Three entry types — use whichever fits.

### Progress note (in-progress)

```markdown
### Update — YYYY-MM-DD HH:MM (progress)

<One paragraph: what you've done since the last update, what you're working on
now, anything surprising. No need to log every micro-step.>
```

### Blocker (stop and wait)

```markdown
### Update — YYYY-MM-DD HH:MM (blocker)

**Blocked on:** <one-line summary>
**What I tried:** <concrete attempts, in order>
**What I need:** <decision | clarification | authorization>
```

### Completion (phase done)

```markdown
### Update — YYYY-MM-DD HH:MM (complete)

**Summary:** <one paragraph: what was built, any deviations from the spec and why>

**Acceptance criteria:** all ticked above.

**Commands:**

```
cargo fmt --all
<paste output>

cargo build 2>&1 | tail -20
<paste tail output>

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
<paste tail output>

cargo test 2>&1 | tail -30
<paste tail output>
```

**End-to-end verification:**

For each command in the phase doc's E2E section, paste the actual output. (If the
phase doc declared E2E N/A, restate the reason in one line.)

**Files changed:**
- `<path>` — <one-line summary>

**New tests:**
- `<test_name>` in `<path>`

**Commits:**
- `<sha>` — <subject line>

**Notes for review:** <anything the reviewer should know>
```

---

## Review and Bug-Report Cycle

When the executor marks a phase **review**, the architect:

1. Reads the phase doc + diff + Update Log completion entry.
2. Runs the commands themselves to confirm they actually pass.
3. Spot-checks the tests are real (not passing via assertion omission).
4. **Re-runs every command in the phase doc's End-to-end section and diffs the
   output against the transcript pasted in the Update Log.** Reading it for
   plausibility does not count — see § "A pasted transcript is a claim, not
   evidence."
5. Either **approves** — flips to `done`, updates the milestone README's phase
   table — or **rejects**, which is the four-step sequence below.
6. **Records a structured review verdict** (below) — at every approval, not just
   when something went wrong. This is the supervision label for model evaluation
   *and* the substrate for human project review. One write, two consumers.

### The bounce sequence — four steps, in order, none optional

A rejection is not "file a bug and flip the status." It is these four, and the
third is the one that actually determines whether the re-dispatch does anything:

1. **Write the bug report** in the milestone's `bugs/` directory.
2. **Flip the phase doc's `Status:`** back to `in-progress`, naming the bug.
3. **Refresh the phase doc's acceptance criteria** so the outstanding work is
   expressed *there*, and **run each new criterion to confirm it fails against
   the current tree**. Paste nothing you did not execute.
4. **Update the milestone README row** and record the bounce in telemetry.

**Step 3 is the load-bearing one, and it is the one that gets skipped.** The
executor evaluates the *phase doc* to decide whether there is work to do; it
does not evaluate the bug doc for that purpose. A phase doc whose every
criterion still passes is a phase doc that certifies itself as finished, and the
correct reading of it is "complete, nothing to do." The bug report is where the
diagnosis lives; the acceptance criteria are where *doneness* lives. Filing the
former without updating the latter leaves the two in contradiction and the
executor obeys the one it was told to obey.

This is why the rule is a numbered step rather than a clause: it previously
existed as a parenthetical inside the compound "rejects" step above, and was
skipped by the architect who had folded it in the day before. A rule that must
be *remembered* at the moment of writing a bug report is a rule with a failure
rate; a rule that is step 3 of 4 is checked off.

The countermeasure applies to a green bounce with particular force — see
§ "Green bounces need a refined re-dispatch, never a plain one" in the escalate
guidance. A loud header, a do-not-touch list, enumerated edits and an inverted
finish condition are all *inside the bug doc*, and M11 phase-07b demonstrated
that all four together are insufficient when the criteria still pass. Round 2
returned `complete` with an empty diff in 31 turns; round 3, with four criteria
confirmed failing, did the work in 82.

*(Folded 2026-08-07 on PE sign-off. Second occurrence of the underlying
empty-diff failure — M11 phase-07a round 2 and phase-07b round 2 — and the first
occurrence of the original prose fold itself failing to be applied.)*

### Review verdict

Append to the approved phase's Update Log:

```markdown
### Review verdict — YYYY-MM-DD

- **Verdict:** approved_first_try | approved_after_N | rejected | escalated
- **Bounces:** <count> (bugs: <id(s)> — <max severity>, or "none")
- **Executor:** <model name>
- **Scope deviations:** <what the phase cut/deferred vs. its spec, or "none">
- **Calibration:** <fold filed / lesson, or "none">
```

Keep it terse — it's a label, not a narrative. The milestone retrospective rolls
these up at close.

### Bug report template

File at `docs/dev/milestones/M<n>-<slug>/bugs/bug-<phase>-<n>.md`.

```markdown
# Bug <n> on phase-<phase>: <One-line title>

**Severity:** blocker | major | minor | nit
**Status:** open | acknowledged | fixed | verified
**Filed:** YYYY-MM-DD

## What's wrong
<Concrete. Quote the offending code with file:line. State observed behavior.>

## What should happen
<Concrete. Reference the architecture doc section or phase spec requirement.>

## Root cause
<Why it happens, at the level of the mechanism — not the edit that fixes it.
"`find` selects one candidate and `and_then` discards it on failure, so the
remaining candidates are never examined." Name the file and symbol.>

## Definition of done
- [ ] <command produces expected output>
- [ ] <test_name passes, and what it must assert>
```

### State the symptom, the root cause and the DoD — not the fix

**The three required sections are What's wrong, Root cause, and Definition of
done. A `How to fix` section is optional, and admissible only when the architect
has actually run the fix.** Otherwise, describe the constraint the solution has
to satisfy and let the executor choose the edit.

This inverts the earlier instinct, which was to prescribe the patch and lean on
the executor to type it. The evidence says prescription is where architects are
least reliable: a prescribed fix is a *system fact* — a claim that this edit,
applied to this tree, produces that result — and it is authored from reasoning
about code rather than from running it. Four of them in one milestone were
wrong. Two of those the executor implemented faithfully and the phase bounced;
one the executor correctly refused, and the finding was withdrawn; one was an
impossible instruction the executor burned an entire dispatch trying to satisfy
before the governor stopped it.

The failure mode is specific and worth naming: **a prescribed fix is trusted
precisely because it is specific.** A vague instruction gets sanity-checked
against the code; a confident code block gets typed in. So the more precise the
prescription, the more damage a wrong one does — and precision is not evidence
of correctness when the author never executed it.

What the executor is reliably good at, given a correct root cause, is finding
the edit. It has the compiler, the linter, the test suite and the actual tree;
the architect has none of those at spec-writing time. Give it the diagnosis and
the finish line.

**When you do include a worked example, it must be quoted from code that
exists**, per § "Verify external APIs against live docs" and the green-bounce
treatment's third part. Quoting an existing pattern is evidence. Authoring a new
one is a guess wearing the costume of evidence — and the executor cannot tell
them apart.

*(Folded 2026-08-06 after four occurrences in M11, on PE sign-off: `bug-02b-1`
Finding 1 prescribed a `read_line` recipe that errors on invalid UTF-8 and
`?`-propagates out of a routine whose contract is "always safe to rerun";
`bug-03a-1` Finding 2 prescribed removing an `.or(Some(0))` fallback that three
tests depended on; `bug-07a-1` prescribed a closure form that its own lint gate
rejects as `redundant_closure`, and a test fixture premised on repetition
outranking brevity when BM25 normalizes by document length — the latter cost a
full dispatch and a `NoProgressStall`. In every case the executor's behavior was
correct given what it was told.)*

### Severity meanings

- **blocker** — phase cannot be merged in this state.
- **major** — must fix before done; correctness or contract violation.
- **minor** — should fix; style, naming, a missing-but-not-critical test.
- **nit** — optional preference; executor may decline with reasoning.

---

## Status Flow

```
todo ──► in-progress ──► review ──┬─► done
                  ▲                │
                  └────────────────┘ (bug report filed)
              ▲
              └─ blocked   (executor waiting on architect)
```

The status lives in the phase doc's frontmatter and is mirrored in the milestone
README's phase table. The two **must** match.

---

## Phase progression & triggers

"Mark a phase done" and "write the next phase" are **separate acts**. Marking
done — flipping the phase to `done`, updating the README phase table, committing
— is a checkpoint. Drafting the next phase is a fresh decision that benefits from
the just-finished work being on disk. Keeping them separate lets the human
inspect before more work is generated.

**Default: gated.** After a review passes, the architect marks the phase `done`
and **stops**. The user advances explicitly. The architect does not draft or
dispatch the next phase on its own. This keeps the review a real gate and the
human in control of scope.

**Milestone boundaries are always a human gate.** When a milestone's in-scope
phases are all `done`, the review skill approves the final phase as normal and
stops — it does **not** write the retrospective or close the milestone. Milestone
close is a separate explicit step: the human invokes `/rexymcp:architect` to write
the milestone-specific retrospective, fold calibration lessons into `WORKFLOW.md`
(with sign-off), and update `NEXT.md` to "none". This is where direction changes
happen; it is never automated by the review step.

**Opt-in autonomous loop (off by default).** For hands-off runs, the user may
start an explicit `/rexymcp:auto` run that chains draft -> dispatch -> review ->
escalate/re-dispatch across phases with **full review rigor and no per-phase
pause** — the review procedure runs verbatim (independent gate re-runs, DoD walk,
telemetry verdict, commit); only the human pause between steps is removed. It is
explicitly enabled per run, never the default, and it **composes** the
interactive skills rather than forking them — a behavior difference between an
interactive and an autonomous run of the same step is a bug. Dispatch drives
`execute_phase`'s **async contract** — it polls `get_run_status` to reap each
spawned run — and a running phase is **interruptible** out-of-band (`rexymcp stop`
for the human, `stop_phase` for the architect between polls), which the loop
treats as a deliberate human signal. The loop stops for the human on: a milestone
boundary (always), any blocker or "What Executors Never Decide" item, exhaustion
of the per-phase assist budget (`[escalation] max_assists` autonomous escalation
round-trips on one phase), the loop-level runaway backstop, or a phase returning
**`cancelled`** (a deliberate `rexymcp stop` / `stop_phase` interrupt — the loop
surfaces the partial work and hands back). Every stop produces a **loop report** — phases run, verdicts,
assists spent, token/cost totals where harvested, and why it stopped — so the
human resumes from a briefing, not a scrollback dig. Every architect activity in
the loop is journaled to the telemetry store; token usage is harvested from the
client's own transcripts where available and recorded as absent elsewhere, never
estimated.

**The executor is a local LLM, not a coding agent.** The model driving phases
through this workflow is a single-purpose executor: it has the project's tool set,
the embedded contract + STANDARDS + the phase doc, and a bounded turn budget. It
does *not* have web access, cannot escalate mid-phase to a stronger model, and
does not negotiate scope. Treat its outputs as the work of a junior engineer who
cannot ask clarifying questions. Mismatched-expectations bugs are *spec bugs*, not
executor bugs.

**Front-load by task shape, not by default.** Whether to pre-inject — and how much
— depends on the kind of work:

- **Design-discovery phases** (the executor must find a load-bearing API or
  architecture constraint the spec does not fully determine): front-load the key
  constraint — the load-bearing seam, the critical API call, a worked example of
  the exact pattern to follow. One focused paragraph beats an exhaustive wall of
  context.
- **Mechanical phases** (move/rename/extract whose shape the spec fully
  determines): normal density; no front-loading needed.

**Lean bias: prefer under-specification over over-specification.** The architect
runs on a cloud model (Claude); the executor runs locally. Every extra token in the
spec costs cloud budget. A bounce from the local executor is cheaper than an
over-specified spec written by Claude. Front-load just enough to prevent the
predictable bounce — not everything you could say.

*(Folded from M2: 6 design-discovery phases drew 7 bounces + 3 escalations under
lean specs; 10 mechanical splits cleared 9-of-10 first try. Discriminator is task
shape, not model size.)*

---

## What Executors Never Decide

- Whether something belongs in core vs. a plugin.
- Whether to add a dependency.
- Whether to change the architecture doc.
- Whether to skip a test, mark it as ignored, or suppress a warning.
- Whether to widen a phase's scope to fix a related issue noticed in passing.
- Whether to deviate from STANDARDS.md "because this case is special."

All of these are blockers. File them in the Update Log and stop.

---

## Calibration — fold lessons in

The workflow this document describes is the product's own workflow, and the plugin
embeds these files verbatim. So **everything learned building a project must be
folded into these docs** — there is no separate place for "lessons learned for
later."

Fold on a **recurring pattern**, not a one-off:

- One occurrence = calibration data; note it, don't change docs yet.
- Two occurrences = trend worth folding; update the relevant doc.
- Three occurrences = the doc was wrong; fold immediately.

Where each lesson lands:

| Lesson | Lands in |
|---|---|
| Executor needs to remember X every phase | `STANDARDS.md` |
| Every implementation should uphold X | `STANDARDS.md` |
| Architect spec-writing / review discipline | this file |
| Phase-doc or bug-report template addition | this file |

The architect revisits both docs **after each milestone closes**, before drafting
the next milestone's phase 01. If no folds are warranted, the milestone README's
Notes section says so explicitly: "M<n> retrospective: no new patterns, no
folds." Silence is not the default.

### Specs pin behavior, not rendering

When writing a phase spec, pin the **test behavior** (what it asserts) and the
**test name** (so coverage is auditable) — but do **not** pin exact test count,
test-file placement, or call-site argument identity. Those are the executor's
structural calls. When pinning a grep literal in the E2E block, pin user-visible
**content**, not source-text rendering (path qualifiers, whitespace nuance,
markdown formatting marks). If you can't decouple content from rendering, use a
prose behavioral assertion and verify by inspection instead of grep.

**Pin negative cases, not just positive ones.** For specs that hinge on
string-matching, path resolution, or escape semantics, the boundary is where the
bugs live: give explicit *must-NOT-match* / *must-stay-hermetic* examples and
require tests for them, not only the positive cases. The executor implements the
spec literally, so an under-specified boundary leaks straight through. (An early
milestone's bounce traced to a positive-only spec — an escape test whose scope
root *was* the temp directory, so "outside the root" wrote outside the sandbox;
and a classifier that matched a shutdown keyword as a bare substring and so
blocked an unrelated command containing that substring. Both would have been
caught by a single pinned negative example.)

### Derive intentionally

Before adding protocol-derived traits to a struct, ask whether it actually gets
serialized at runtime. If yes, add them — they're load-bearing. If no, omit them;
an unused derive can force upstream additions on shared types and push the executor
into unauthorized edits of settled phases.

The same applies to **wired-in state, not just derives**: don't have a phase
record into something whose consumer doesn't exist yet. Either pin the consumer
in the same phase, or defer the write until the phase that consumes it.

**Wrap-vs-derive at protocol boundaries.** When exposing a type at a protocol
boundary (tool output, log line, telemetry record), the boundary trait has to
apply to *every* type in the schema tree. Two ways to satisfy that:

- **Derive directly** when the schema tree is small and locally-owned. The
  output type is one struct of primitives the architect controls; adding the
  derive is a one-line edit, no upstream cascade.
- **Wrap in a single-field generic carrier** when the schema tree is large or
  foreign. The wrapper struct derives the boundary trait; the inner carrier
  carries the pre-serialized payload, so no derive has to be added to the foreign
  types.

Cost trade-off: wrapping adds one nesting layer in the output; deriving forces the
boundary trait on every type in the tree. Choose at draft time per type, not at
code time.

### Anticipate cross-boundary trait bounds

When a phase introduces a new protocol or async boundary (tool, async runtime,
persistence), **enumerate in the spec the trait bounds the boundary will require**
and check at draft time whether the types crossing the boundary already satisfy
them. If they don't, the spec either authorizes the narrow upstream edit to add
the bound, or pins the wrapper pattern to sidestep it.

The cost of missing this at draft time is repeating one of two failure modes:
(1) the executor discovers the missing bound mid-phase, files a blocker, and waits
for architect authorization; or (2) the executor adds the bound without
authorization and the architect catches it at review as a scope deviation. Both
end in the right place, but both cost a round trip.

### Verify external APIs against live docs

When a phase references an external API the architect cannot live-verify
(an SDK's macro names, a protocol's wire format, a CLI's config schema, a
plugin manifest shape, a third-party library's surface), the spec MUST
include a **Pre-flight step** instructing the executor to verify the
specifics against the live documentation and **trust the docs over the
architect's sketch**.

The architect's reference sketch in such specs is the *intent* and
*behavior* the phase pins; the *exact* field names, macro forms, file
paths, and frontmatter shapes are the executor's to discover and adapt.
Any divergence between sketch and live docs the executor cannot resolve
from the phase doc is surfaced as a **blocker** (returned to the architect
as a briefing — the executor is headless and cannot ask inline), not a
silent fix during execution. The architect responds with a refined spec or
amendment and re-dispatches. A divergence the executor *can* resolve from
the supplied reference is adapted cleanly and recorded in "Notes for
review" rather than blocked on. **A blocker is cheap; a wrong silent fix is
expensive.**

Pair this with the **declare-deviations** discipline: even when the
executor adapts cleanly to the live docs (the right call), the
adaptation is named in "Notes for review" so the architect can update
their mental model of the API for future specs.

The **Pre-flight step's shape**:

> N. **Verify the current `<external API>` <thing>** before coding. The
>    architect cannot reliably enumerate the exact `<field/macro/path/
>    shape>` and the sketch in § X below may be wrong. Sources to consult,
>    in priority order: the official docs site; the upstream source / tool
>    introspection; working examples from other consumers. **Trust the
>    docs over the sketch.** Pin the *behavior* this phase requires; let
>    the executor adapt the *structure* to the real convention. Flag any
>    divergence in "Notes for review".

Use this step whenever the phase touches an external library, a third-
party manifest format, a tool's CLI flag set, or any other surface the
architect can't introspect from inside their own session. Skipping it
when it applies is how silent improvisations enter the codebase.

### Prefer additive change shapes; avoid wide-blast-radius breaking changes

When a phase requires modifying a type used at many call sites (an enum variant,
a function signature, a trait method), the architect must choose whether the spec
asks the executor to **mutate** the existing symbol or **add** a new one.

**Mutation is high-risk** when the type has many call sites: every site stops
compiling the moment the definition changes, the executor must update all of them
before the build is green again, and the verifier's consecutive-failure limit (3
strikes) can fire before the cascade completes — leaving the codebase in a
broken-in-progress state. The more call sites, the narrower the window.

**Additive shapes sidestep this entirely.** A new enum variant, a new struct field
with `#[serde(default)]`, a new function that takes the role of the old one — these
keep the codebase compiling at every step. Only the *new* code needs updating; the
old code keeps working until it is deliberately migrated.

**At draft time, before speccing a multi-site mutation, ask:**
- Is there an additive shape that achieves the same behavioral goal?
  - Add a *sibling* variant instead of changing the existing one?
  - Add a *new* field with `#[serde(default)]` instead of widening an existing
    field's type?
  - Add a *new* function and migrate callers one-by-one instead of changing the
    signature of the current one?
- If mutation is unavoidable, can the blast radius be bounded to ≤ 3 sites (within
  the verifier's retry budget)?

If yes to either, use the additive shape and pre-inject it. If the blast radius
exceeds ~3 sites and no additive alternative exists, flag it explicitly in the phase
doc and instruct the executor to `cargo build` after **each individual site** before
moving to the next.

**What to pre-inject when a multi-site change is unavoidable:**
Give the executor a `grep`-verified complete list of every site, in the order to
update them, with a "build after this site" instruction after any site that would
break a separate file. An incomplete list is how this class of failure happens — the
executor changes the definition and runs out of runway.

*(Folded from M7/phase-05b: two hard_fails of the same class — breaking a
multi-site type change — on two separate phases, Qwen3.6-27B-FP8. Additive
restructure resolved both.)*

### Post-write formatting is a runtime concern, not a spec concern

When a formatter (`ruff format`, `gofmt`, `rustfmt`, etc.) is part of the
project's command set, a recurring class of verifier hard-fail arises: the
executor runs the formatter during its turn loop, then issues a subsequent
`write_file` that overwrites the formatted file with unformatted content.
The verifier fires on the unformatted file, produces 3 consecutive failures,
and halts with a hard_fail.

**Root cause:** The executor's tool-call loop is not atomic with respect to
formatting. Any `write_file` issued *after* the format step undoes it.
The executor is not buggy — it formatted correctly; it simply continued
working and overwrote the result.

**What does not work:** Spec-level "Completion checklist" instructions to
run the formatter before `git add`. M1/phase-03 of mp3-player pre-injected
this instruction explicitly; the executor ran it, then issued another
`write_file` afterward. A spec instruction cannot prevent a later write.

**The fix is runtime-level:** The rexyMCP runtime should run the project's
`format` command (and optionally `lint --fix`) as a **post-write,
pre-verifier hook** after each turn where files were written to disk. This
makes formatting unconditional and turn-ordering-independent. Filed as a
runtime feature request against rexyMCP.

**For the architect:** Do not add "run the formatter" steps to completion
checklists in phase specs — proven ineffective for this failure class.
Apply the formatting fix manually on close-out until the runtime hook lands.

*(Folded from M1/mp3-player: four phases (01×2, 02, 03) on
google/gemma-4-12b hit the same ruff formatting verifier halt. Spec
instruction pre-injected in phase-03 — still failed, confirming the fix
must be runtime-side.)*

### Executor self-sabotage on delete-heavy rewrites is a runtime concern

Two distinct executor pathologies recur on phases that **rewrite a load-bearing
path** — deleting and replacing existing functions rather than adding new ones.
Both are runtime (tool-loop) failures, not spec gaps, so — like post-write
formatting — the durable fix is runtime-side, not a spec instruction.

**1. Git-thrash: the executor reverts its own uncommitted work.** Mid-phase, the
executor runs `git checkout`/`git stash`/`git reset` on files it just wrote,
discarding a correct implementation, then loops in confusion because its diff
"disappeared." A spec instruction cannot prevent this (the executor is not
reading the spec at the moment it decides to run git), and the existing runtime
guard is **advisory and incomplete**: it prints "Do not revert your own work" for
`git checkout <file>` but does not *block* it, and does not cover
`git checkout HEAD -- <file>` or `git stash`, both of which the executor used to
wipe its work anyway.

**2. Verify-loop: the executor re-runs the same read-only check indefinitely.**
Chasing a test/grep result, the executor issues near-identical
`grep`/`cargo test` calls with no file writes between them, making no progress.
The identical-call/oscillation governor catches the *exactly*-identical case
(6 identical calls → hard_fail) but misses near-identical variants — one such
loop ran 529 turns until a human `rexymcp stop`ped it.

**The runtime fixes (feature requests against rexyMCP):**
- **Hard-block, don't warn**, any `git checkout|restore|reset|stash` that would
  discard the executor's *own* uncommitted changes from this run — covering the
  `HEAD -- <path>`, bare-`<path>`, and `stash` forms. Better still: auto-create a
  throwaway checkpoint commit before allowing it, so nothing is ever lost.
- **Broaden the loop governor**: normalize whitespace/argument ordering before the
  identical-call comparison, and trip on *N consecutive read-only calls
  (`grep`/`git status`/`cargo test`) with zero intervening file writes* — the
  signature of a verify-loop that makes no progress.

**Until the runtime lands these, the architect mitigation (proven twice):**
- **Split delete-heavy rewrites away from additive work.** A phase that both adds
  a new module *and* rips out an old path is where the executor thrashes. Split
  it: an additive phase (new module/functions, nothing deleted) followed by a
  rewire phase (delete + wire). The additive half never triggers the pathology,
  and when the rewire half fails, the additive code is already safely on disk.
- **Prefer takeover over resume/re-dispatch when the executor loops and correct
  code is already on disk** — resume just re-enters the same loop. Salvage the
  intact files; reconstruct only the mangled one (restore from HEAD, reapply the
  intended deletions).

*(Folded from M4: git-thrash on phase-01 and phase-03; verify-loop on phase-05a
(governor-caught) and phase-05b (evaded the governor, human-stopped at 529
turns). All Qwen3.6-27B-AEON. The 05a/05b split contained the blast radius both
times — the executor's additive files survived; only the delete-heavy file
needed architect reconstruction.)*

### A NoProgressStall is usually a nearly-finished phase — diagnose the tree before choosing a lever

The read-only-stall governor asked for in the fold above **now exists and works**
(`read_only_stall_threshold`). It fires cleanly on the pathology it was built for:
the executor writes most of a phase, then loops re-reading one file instead of
running a gate. What the governor cannot tell you is *how much of the phase is
already correct* — and the answer is consistently "most of it."

**So on any `NoProgressStall` hard_fail, before picking a lever, run the gates
against the partial tree yourself.** `cargo build`, then the lint, then the tests.
This takes a minute and decides everything:

- **It tells you which lever is right.** If the missing piece is small and the
  executor simply ran out of turns, **resume** with the diagnosis — that is the
  cheap path and it preserves the model-vs-spec data point. If the missing piece
  is *the very edit it stalled on*, **take over** — a resume re-enters the same
  wall, and verifying a patch precisely enough to specify it safely means writing
  it anyway, at which point handing it back to be retyped adds risk and yields no
  data.
- **It finds defects nobody else will.** The executor stalled *before* running any
  gate, so its partial work is unverified by construction. Across three stalls
  this surfaced a self-contradictory assertion (a test asserting a file was absent
  and then reading it), an unused import failing `-D warnings`, and a dead_code
  error from a second `mod harness;`. None of these were visible in the briefing.

**Do not treat the stall itself as evidence the work is bad.** In all three cases
the executor's *design* was right — the seam it was asked for, the table, the
gate — and what was missing was integration into a large existing file.

**On prevention, be honest:** the fold above established that runtime pathologies
need runtime fixes and that spec-level instructions are ineffective against them.
Nothing here contradicts that. An anti-stall line in resume guidance ("after each
edit, run the one command that checks it; never re-read a file you just wrote")
coincided with a clean 57-turn completion once, but that run also supplied the
missing diagnosis, so the instruction is unproven on its own. What *is* supported:
all three stalls happened while integrating into a large pre-existing file, so
front-loading the exact integration point — the enclosing function, how values
reach it, the surrounding lines — reduces the surface the executor has to
rediscover.

*(Folded 2026-07-30 after three occurrences in M6: phase-04 (stalled re-reading
`path_audit.rs` after landing `classify_text`), phase-06b (stalled after writing
the whole phase, gates never run), phase-08 (68 consecutive reads of
`daemon/mod.rs`, daemon integration never written). Partial work was correct in
all three; two resumed to completion once given the diagnosis, one needed
takeover because the stalled-on edit *was* the remaining work.)*

### Validation features depend on the target toolchain — verify availability at design time

Validation features (a verifier that runs the project's checker, code-intelligence
features like find-references or compiler-suggested-fixes) shell out to
**per-language toolchains** the executor host must actually have. They split into
two tiers, and the tiers answer "fail open or fail hard?" differently:

- **Tier 0 — the `cargo build` / `cargo test` / `cargo clippy --all-targets --all-features -- -D warnings` /
  `cargo fmt --all` toolchain.** Language-agnostic, user-configured, and
  **already a hard requirement**: a phase cannot reach `done` without build/test
  passing (STANDARDS §1). **This is how the project supports *any* language** —
  point the command set at the language's tools and the loop + DoD gates work,
  even for a language with no dedicated verifier.
- **Tier 1 — validation *enhancers*** that **augment** Tier 0 with incremental,
  structured feedback. The loop **degrades gracefully** to Tier-0-only without
  them. Enhancers backed by a *compiled-in library* (e.g. a bundled parser grammar)
  need **no** machine install; only enhancers that **shell out to a binary** are a
  runtime-availability concern.

**Fail-open at runtime; fail-hard-*advisory* where a human is present.** The
deciding axis is *who can act on a missing tool, and when*:

- **At the human-present boundary** (project bootstrap / first design session):
  detect missing toolchain binaries and **present a resolution plan** — install
  instructions, or scope the feature to the languages whose toolchain is confirmed
  present and defer the rest. The user chooses; advisory, not a refusal.
- **At runtime inside the headless executor**: a missing binary must **degrade to
  a model-visible advisory that names the binary and the remedy** and let the
  executor keep working — never a panic, never an opaque "spawn failed", and never
  an outcome the verifier governor counts as a *failure strike* (a missing tool is
  a skipped/advisory outcome, distinct from "the tool ran and found errors").

**When drafting a phase that adds or extends a validation feature, the architect
must:** (1) enumerate the runtime binaries it invokes (name + minimum version +
the exact flags / machine-readable format it parses), distinguishing compiled-in
libraries from machine binaries; (2) confirm they are present and emit that format
— or instruct a Pre-flight check; (3) if a binary is missing for a target
language, inform the user with a resolution plan before shipping a feature that
would only degrade; (4) pin the missing-binary runtime behavior in the phase doc
as a named advisory, per the rule above. Record the feature's toolchain
dependencies in the phase doc (Pre-flight or a "Toolchain dependencies" line).

### Run every count criterion; never derive it

A phase doc that pins a count (`grep -c … returns 4`) is making a claim about the
tree. **Run the command and paste its answer. Never compute the number by
reasoning about the code.** Derived counts are wrong often enough that the
practice of running them has caught an error in roughly a third of the phases
that used it — while the phases whose counts were reasoned out produced four
separate miscounts, one of which reported success against an unmet goal.

Two corollaries, both learned the hard way:

- **Text-based greps count prose.** A doc comment, an assertion message, or a
  phrase inside the spec you are writing will match. Four separate criteria in
  this project were off by one for exactly this reason — including one where the
  spec instructed the executor to write a comment that then broke the spec's own
  count. When a count comes back higher than expected, look for prose before
  assuming code.
- **The instrument can be blind.** `grep -c "sessions\.lock()"` cannot see a
  multi-line split, and it cannot see the same call on a differently-named
  variable. Both blind spots reported clean while real sites remained. Before
  pinning a criterion, ask what the instrument *cannot* see; where the type
  system can enforce the property instead, prefer that and retire the grep.

**Re-run the block before dispatch, not only at drafting.** A number that was
correct when the phase was written goes stale as soon as an earlier phase edits
the file — one M5 phase had all eight of its sites shift by −3 between drafting
and dispatch, and every line number in its spec was wrong before the executor
ever read it. Numbers age; re-run, don't trust. For the same reason, a phase doc
that lists line numbers should say they are current-as-of-drafting and point at
the command that re-derives them.

*(Folded 2026-07-27 after four miscounts and two blind instruments across M5.
Staleness clause added the same day, after a fifth miscount **in a phase drafted
under this rule** — the rule was not unclear, it was not followed. If a sixth
occurs, the remedy is a mechanical pre-dispatch check, not stronger prose.)*

### A sweep's scope is its convertible sites, not its matches

When a phase applies one mechanical change to every instance of a pattern — a
call wrapped in an adapter, an API migrated, a symbol renamed — the match count
is where scoping **starts**, not where it ends. A hit can be real, correctly
matched, and still not convertible:

- **Its enclosing function is synchronous** and the conversion is `async`.
  Converting it is `error[E0728]`, and the real fix changes a signature, every
  call site, and any test that calls it directly.
- **It is inside a `Drop` impl**, which cannot be `async` at all.
- **It is already in the target form** — an `async fn` needs no off-runtime
  wrapper, and wrapping one hands the blocking pool a future nobody polls.
- **Its expression extends far past the line it starts on**, so a slice boundary
  drawn by line number cuts a single site in half.

Each of these mis-scoped a phase in M5's tmux sweep. The remedy is cheap:
**write the classifier, not just the counter.** For every hit, report its
enclosing function and whether that function is `async`; then put the
unconvertible ones in the phase doc **by name**, as a do-not-convert list with
the reason for each. That table costs a few lines and buys two things — the
executor does not spend turns discovering `E0728` and then "fixing" it by adding
`async`, and the finish condition can be an exact residue ("the scan reports 4,
and all four are on the list") instead of an unreachable zero.

**Sites needing a restructure go in a restructure phase**, not bundled into a
mechanical sweep. Mixing the two is what makes a sweep stall: the phase looks
uniform, so it gets sized as uniform, and then one site consumes the run. This
is the same split that moved blocking-work sites out of the lock-conversion
phases earlier in the same milestone.

*(Folded 2026-07-27 after four mis-scopings in M5's tmux sweep — two sync
enclosing fns, two `Drop` impls, one already-`async` helper, and one expression
spanning a slice boundary. Distinct from the blind-instrument corollary above:
there the instrument was wrong, here it was right and the site still was not in
scope.)*

### Coverage claims are inadmissible without mutation proof

**Never write "test X guards line Y" in a spec, a review, or an Update Log unless
the claim has been demonstrated by mutation** — break the line, watch that test
fail, restore it, watch it pass, and quote the pair.

This project produced three false coverage claims before the rule existed. All
three were plausible, all three were wrong, and one took a mutation at review to
disprove. The mechanism is usually a **fixture default**: a shared `make_*`
helper that initialises a field to the value the assertion checks for makes every
assertion on that field tautological, and nothing in the gate set can see it.

Two rules follow:

- **A spec must not name the discriminating test.** Naming it plants the
  conclusion; the executor then reports what the spec suggested rather than what
  it observed. Require the demonstration instead.
- **"The tests pass" is admissible. "The tests would catch a regression here" is
  not** — unless the mutation pair is quoted alongside it.

When reviewing a phase whose deliverable *is* coverage, **re-run the mutations
independently.** A claimed mutation check is not one.

*(Folded 2026-07-27 after three false coverage claims and one confirmed
fixture-default trap.)*

**Confirm the property is observable before pinning it.** The mutation rule above
assumes a broken line *can* be detected. Sometimes it cannot — and then the spec
has asked for a test that cannot exist, so what comes back is a test that passes on
unrelated grounds:

- **When a spec names a branch, describe a sequence that reaches it** — not merely
  the value it returns. If two branches return the same value, a test asserting that
  value proves nothing about which one ran. One such test was named for an EOF
  branch and passed via a write-failure arm returning the same variant; the branch it
  named was never executed.
- **When a spec pins an observable property, verify it is observable at all.**
  Insertion order into a `BTreeMap`-backed map is not visible in its serialized
  output, so a spec pinning "these keys lead the line" pinned nothing; the test that
  "asserted" it passed on alphabetical ordering the serializer applies regardless.

Both failures look like satisfied criteria and pass every gate. **The tell is that
the mutation does not fail the test** — which is why the mutation must be run, and
run by the reviewer.

*(Folded 2026-07-30 after a third occurrence. The three: a fixture default making
assertions tautological; a test named for a branch it could not reach; a spec
pinning a key order the serializer discards. All three were architect-authored — the
executor implemented what was specified in each case.)*

**A guard's premise must be demonstrated, not described.** The failures above are
all about what the *assertion* can see. This one is about the *fixture*: a test
for an early-return guard passes trivially whenever the input would have produced
the same result by some other path, so the guard is never reached and its removal
changes nothing.

The shape is always a negative assertion — "returns `None`", "the row is absent",
"nothing is emitted" — paired with a fixture chosen to be *inert* rather than
*near-miss*. A low-signal-query guard was tested with a query sharing no token
with the seeded corpus: the search matched nothing, the function returned `None`
through its no-hits path, and deleting the guard entirely left all twelve module
tests green.

**Seeding a non-empty fixture is not sufficient. The fixture must be one the
input would otherwise match.** State it in the spec as a near-miss: name the
input, name the seed, and say *why the seed is reachable* — "`\"hi by\"` is below
the signal floor on every term, and the seeded body contains
`highlight_by_service`, which FTS5's tokenizer splits on the underscore so the
token `by` is indexed and matched." That sentence is what makes the guard
observable.

**And a comment stating the intent is not the demonstration.** The 07b fixture
carried the comment *"seed a non-empty matching corpus so the test is about the
guard, not about an empty index"* — correct intent, wrong fixture, and the
comment made the defect harder to see rather than easier, because it read as
evidence the concern had been handled. **Require the mutation pair, not the
rationale.** Any guard or exclusion criterion must carry a both-directions
mutation in the phase doc: remove the guard, show the named test fails, restore
it, show it passes.

*(Folded 2026-08-07 on PE sign-off, at the third occurrence of the vacuous-guard
family: an identity criterion satisfied by `line.contains("turn")`, a JSON key
every record carries; an exclusion test that passed *with its mutation applied*
because an unrelated empty corpus had wiped its fixture; and this one. All three
were caught only by a reviewer running the mutation.)*

**When a test depends on the fixture being in a particular order, assert that
order in the test.** The cases above are all "the property could not be
observed". This is the neighbouring one: the property is observable, the fixture
is non-empty, every assertion is real — and the code path under test is still
never entered, because the *ordering premise* the fixture rests on is false.

A fixture built so that "candidate A is tried first, fails, and B is used
instead" is only testing the fallback if A really does come first. When that
order is decided by something the spec author reasoned about rather than ran — a
ranking function, a sort comparator, a hash iteration, a directory listing — it
is a system fact like any other, and it is wrong often enough to matter. Get it
wrong and B is tried first, succeeds, and the test passes without the fallback
ever running.

The remedy is one line and it is cheap enough to apply by default:

```rust
// Precondition: the unresolvable hit really is first. If ranking ever
// changes, fail loudly here instead of passing vacuously.
let hits = index::search_turns(query, 8, None);
assert_eq!(hits.first().map(|h| h.turn), Some(100), "fixture precondition: …");
```

Asserting the premise converts a silent false pass into a loud, self-describing
failure — including years later, when whatever decided the order changes and
nobody remembers the test depended on it.

*(Folded 2026-08-06 after M11 phase-07a round 3, on PE sign-off. The fixture was
built on "repeating a term makes a document rank higher"; BM25 normalizes by
document length, so the shorter exact-match body outranked the longer repeated
one, the fallback never ran, and the test passed identically with and without the
fix it existed to cover. It cost a full dispatch: the executor could not make the
required mutation fail, and stalled after ~45 consecutive runs of that one test —
which is the pathology working correctly, refusing to certify an unfalsifiable
guard.)*

### A pasted transcript is a claim, not evidence

**At review, re-run every command in the phase doc's End-to-end section and diff
the result against what was pasted.** Reading a transcript for plausibility is not
verification: a fabricated transcript is *built* to read as plausible, and the
gate set cannot see it — all four gates stay green, because nothing the executor
wrote into a markdown file affects a build.

The three shapes this takes, all observed:

- **Paraphrase in place of a quote.** The Update Log describes what the command
  showed instead of showing it. Cheapest to spot: grep the entry for the command
  string and for a prompt/exit-code marker; if neither is there, no transcript was
  pasted.
- **A splice inside an otherwise-real transcript.** The dangerous one. A 25-line
  block where 24 lines came from a real run and one was copied from a neighbouring
  file with a field swapped. There is **no** reading of that block that reveals
  the bad line — only re-running and diffing does.
- **Results asserted in the completion summary** while the Update Log has only a
  "(started)" stub. Looks complete in the summary the reviewer reads first.

**A true claim in a hand-made transcript is still a failure.** In every observed
case the underlying behavior was correct and the numbers were accurate — the
command worked. What was missing was the evidence chain, which is the only part
that survives to the next reader. Approving on "the claims check out" trains the
next transcript to be written rather than captured.

**Two rules follow:**

- **Executor:** capture mechanically (§ "End-to-end verification"). Never
  hand-assemble.
- **Reviewer:** re-run and diff. "The transcript looks right" is inadmissible;
  "I re-ran it and it matched" is the finding. This is the same discipline as
  § "Coverage claims are inadmissible without mutation proof" — and for the same
  reason: the check that matters is the one the author cannot fake by writing
  more convincingly.

*(Folded 2026-07-30 after three occurrences in M6, all in the same two phases.
Phase 03 bounced twice — first for paraphrase, then for a spliced line — and had
to be finished by architect takeover; phase 04 then asserted its E2E results in
prose with no transcript at all. Each occurrence cost a full dispatch-and-review
round trip. The splice was caught only because that review had been told
out-of-band to re-run and compare; nothing in these docs required it, which is
what this fold repairs.)*

### Every acceptance criterion must be satisfiable, and its mechanics pinned

Both `NoProgressStall` hard-fails in M5 were caused by a criterion the executor
could not satisfy or could not verify — not by an implementation it could not
write. In each case the executor did the work correctly and then burned sixty
read-only turns fighting the criterion.

Before dispatch, **re-read every acceptance criterion against the body of your own
spec** and confirm the spec does not instruct the executor to violate it. The two
failures:

- **Contradiction.** A criterion required an import count to stay at 1 while the
  spec's own tasks converted the only things that used it. Green was impossible.
- **Under-specification.** A criterion said "`git diff` touches no line outside
  `mod tests`" without naming a baseline — but the executor commits as it works,
  so a bare `git diff` said nothing about committed work.

So: **if a criterion asks the executor to prove a property of its own diff, pin
the baseline commit and the exact command.** And if the criterion depends on a
particular mechanism (restore this file, compare against that commit), **name the
mechanism and confirm the harness permits it** — a refinement built around a
command the shell guard blocks is not a refinement. That one cost a run even after
the diagnosis was correct.

*(Folded 2026-07-27 after two hard-fails and one pre-dispatch catch.)*

**A bounce makes the acceptance criteria stale, and stale criteria certify the
phase as finished.** After a review rejects a phase, the criteria in the phase
doc describe the tree the executor already built — so they all pass. Re-dispatch
against them and the executor evaluates the phase doc's own definition of done,
finds it satisfied, and reports `complete` with an empty diff. That report is
honest; the spec lied.

So **the bounce is not finished when the bug doc is written.** Before
re-dispatching, edit the phase doc so that:

- the outstanding work appears **as acceptance criteria**, and
- **each of those criteria fails against the current tree** — run them and
  confirm it, the same way § "Run every count criterion" requires for any other
  pinned number, and
- any count that the fix will change is re-pinned to its new exact value. "More
  than 1128" was satisfied by the 1135 already on disk; "exactly 1136" is not.

A bug doc is a supplement, not a substitute. The executor reads the phase doc to
decide whether there is work to do; that is the document a bounce has to
invalidate.

*(Folded 2026-08-06 after M11 phase-07a round 2, on PE sign-off. First
occurrence, folded on request rather than on recurrence: the failure is silent
and self-certifying — every gate green, a clean tree, an accurate completion
summary — and the only signal that anything was wrong was an empty diff.)*

### A phase that exhausts a trait's uses must say what happens to its import

When a phase converts *every* call site of a trait method, the trait's import
becomes unused — and **`cargo build` and `cargo clippy --all-targets` disagree
about whether that matters.** For a test-module import, build reports zero
warnings while clippy errors. Two hard-fails in this project trace to that
disagreement.

**Never assert an import count without first checking whether the phase's own
edits exhaust that trait's uses.** Count the remaining uses *after* the planned
conversions, not before. If the count reaches zero, authorise the deletion
explicitly; if it does not, say which surviving uses keep it alive and where they
are, so the executor does not delete it on the "last five phases ended in a
deletion" prior.

**Treat clippy as authoritative for import liveness**, and run gates bare — a
command piped through `tail` exits with `tail`'s status, so a failing gate reads
as passing.

*(Folded 2026-07-27 after two hard-fails on the same disagreement.)*
