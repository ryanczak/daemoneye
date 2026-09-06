# TODO — cross-milestone items

Standing items that outlive any one milestone. Not phases: a phase belongs in a
milestone directory with a spec. These are decisions or tooling gaps parked
deliberately, with enough evidence attached that whoever picks one up does not have
to re-derive the case.

---

## 1. Mechanise the pre-dispatch acceptance-criteria check

**Status:** open — logged 2026-07-30 at the M5 close (PE decision: "explore how to
realize it later").
**Origin:** M5 retrospective, § "Folds proposed for `WORKFLOW.md`", fold 1. Fold 2
of that pair was applied; this one was deliberately **not** written as prose.

### The problem

Across M5, **eight acceptance criteria were defective**, and **three cost a run** —
110 turns and 60 turns on two `NoProgressStall` hard-fails, plus one earlier. In
every case the executor had already completed the implementation and passed all four
gates, then burned its remaining budget fighting a criterion it could not satisfy or
could not verify.

**Not one `hard_fail` in M5 traced to code the executor could not write.**

The two recurring shapes:

- **A negative phrased over a grep that legitimately matches something is
  unverifiable.** "shows no new occurrence", "does not match the X call" — with
  pre-existing hits there is no output that distinguishes pass from fail. Three of
  the eight.
- **A criterion over the executor's own diff must pin a baseline.** The executor
  stages and commits as it works, so a bare `git diff` returns nothing. Two of the
  three run-costing defects were this.

### Why prose is not the fix

This is the crux, and it is why the item is parked rather than folded.

`WORKFLOW.md` § "Run every count criterion; never derive it" **already** says to pin
the baseline, and its dated note **already** predicted the remedy:

> *If a sixth occurs, the remedy is a mechanical pre-dispatch check, not stronger
> prose.*

We reached eight. The rule was not unclear — it was not followed, twice, by an
architect who had read it. Adding a paragraph restating it would be the fourth
attempt at the same intervention.

### Sketch of a mechanical form

Not a design, just the shape the evidence points at. The `## Acceptance criteria`
block is already machine-readable enough: criteria are checkbox lines, and most
embed a shell command in backticks with an expected value in the prose.

A dispatch-time check could:

1. Extract the fenced/backticked commands from the phase doc's
   `## Acceptance criteria` section.
2. Run each against the tree **before** handing the phase to the executor.
3. Report, per criterion: *runs and passes now* (suspicious — the phase may be a
   no-op), *runs and fails now* (expected — this is the work), or **does not run /
   is ambiguous** (a spec blocker; refuse the dispatch).

Category 3 is the whole value. All eight M5 defects were category 3 or a
wrong-expected-value that a dry run would have exposed.

Open questions worth deciding before building anything:

- Where does it live — the `rexymcp` binary at dispatch time, or an architect-side
  step? Binary-side makes it unskippable, which is the point; architect-side is
  cheaper and can be advisory.
- How are expected values expressed so a machine can compare them? Today they are
  prose ("returns **3**", "**1**, unchanged"). A convention would be needed, and
  imposing one on the phase-doc format is a real cost.
- What about criteria that legitimately cannot run pre-dispatch (E2E against a real
  binary, "test X passes" for a test that does not exist yet)? These need an explicit
  opt-out marker, or the check will cry wolf on every phase.

**Note:** this would be a change to rexyMCP itself, not to DaemonEye. It is logged
here because DaemonEye is where the evidence accumulated; the work belongs in the
rexyMCP repo.

### M20 evidence — occurrences 9–12 (appended 2026-09-06, PE decision)

M20 produced four more, reviewed at the milestone sign-off. **Deliberately not
folded as prose into `WORKFLOW.md`:** this item already establishes that the rule
was stated and unfollowed, and that a restating paragraph would be the fourth
attempt at the same intervention. These are logged as evidence for the mechanical
check, not as a new pattern at threshold three.

| # | Where | Criterion | Pinned | Actual | Shape |
|---|---|---|---|---|---|
| 9 | phase-01 | `grep -c "pub execution: ExecutionConfig" src/config/types.rs` | 2 | max **1** | unreachable value |
| 10 | bug-02-3 | unanchored `grep -c 'fn parse_outcome …'` | 1 | **2** | self-match |
| 11 | phase-05b r1 | three `grep -c 'fn <name>'` counts | 1 each | **7 / 3 / 2** | unreachable value |
| 12 | phase-06a | `pgrep -fc 'daemoneye shell-host'` | 0 | **1** on a clean tree | self-match |

Verified at sign-off rather than relayed: `types.rs:44` `pub execution:
ExecutionConfig,` matches and `:69` `execution: ExecutionConfig::default(),`
does not, so 2 was unreachable; and `fn parse_outcome` appears **7** times in
`src/shell/pty.rs` and **5** times in `bug-02-3.md` itself, so the unanchored
grep could never return 1.

**Two shapes M5 did not have**, both worth a check knowing about:

- **Unreachable expected value** (#9, #11). The criterion runs and returns a
  number; that number simply cannot become the pinned one.
- **Self-match** (#10, #12). The probe matches its own context — the criterion's
  text inside the doc it lives in, or the `pgrep` pattern inside the command line
  of the shell running it. This generalises past greps to any probe executed in
  an environment that contains the probe.

**The distinction this evidence sharpens, and the reason dry-running has not been
enough:** running a criterion pre-dispatch proves it **fails now**. It does not
prove it **can pass later**. Those are different checks, and only the first has
ever been done by hand. Every one of #9–#12 failed correctly before dispatch; all
four were still defective. A pre-dispatch check that only reports *runs and fails
now* would have passed all four — so the check's category 3 (**does not run / is
ambiguous**) needs a fourth category: *runs, fails now, but the expected value is
unreachable or the probe matches itself*.

**One positive data point.** #12 was caught **before** dispatch, because the
criterion was run in the context it would actually execute in — a `bash -c`
wrapper whose own command line contained the pattern. The practice works when
applied; it is not reliably applied by hand, which is the argument for
mechanising it rather than restating it.
