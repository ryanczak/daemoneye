# Bug 1 on phase-03c: a new test failed in the filed three-consecutive-runs evidence, and the capture cannot say which

**Severity:** major
**Status:** open
**Filed:** 2026-09-13

## What's wrong

The phase's acceptance criteria require *"Full lib suite **1841 or more**,
`0 failed`, on **three consecutive runs**"*. **The filed capture does not show
that.** Section E of the pasted transcript — verified byte-identical to
`/tmp/e2e-m22-03c.txt` at review, 4818 bytes — reads:

```
== E. lib suite, three consecutive runs ==
test result: FAILED. 1840 passed; 1 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.14s
test result: ok. 1841 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.07s
test result: ok. 1841 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.11s
```

One of the three runs failed. A later Update Log entry supplies three further
clean runs, but a supplementary rerun does not satisfy a criterion whose whole
purpose is to catch an **intermittent** failure: the criterion worked, and the
answer it gave was "this suite is not reliably green."

**The failing test cannot be identified from the evidence.** Section E of the
block pipes through `grep '^test result'`, which keeps only the summary line
and discards both the `<name> ... FAILED` lines and the trailing `failures:`
block. Re-reading the run's session logs at review confirms it: every
`1840 passed; 1 failed` record carries no test name. The one place a name does
survive anywhere in the logs is a single
`daemon::executor::knowledge::shell::tests::find_in_shells_searches_only_scoped_shells ... FAILED`
— **a test this phase introduced** — but it cannot be tied to the failing
full-suite run from what was captured.

**Everything else about the phase is correct** and was verified independently
at review: 42 tools / 30 core, `Category::Read`, not approval-gated,
`enum_values` untouched, `pane.rs` and `src/shell/` unchanged, the shared
resolver single (`Accepted values:` → 1) with the hardcoded tool name gone
(`unknown list_shells scope` → 0), and **all 15 pre-existing
`knowledge::shell` tests byte-identical** across the refactor (checked
function-by-function against `d24a13ba`). Both mutation checks reproduce by
hand: the per-shell cap mutation reddens
`find_in_shells_caps_total_matches_across_shells` alone (20 passed, 1 failed),
and aborting on an unreadable shell reddens
`find_in_shells_skips_an_unreadable_shell` alone (20 passed, 1 failed). The
paste is faithful and the failure was **disclosed, not hidden**. This bug is
about an unmet criterion and an undiagnosable flake, not about wrong code.

**Not reproduced at review:** 10 consecutive full-suite runs, all
`1841 passed; 0 failed`. Observed frequency is therefore roughly 1 in 16+ runs
across this review and the executor's own session — low, which is exactly what
makes it worth pinning down now rather than after it starts failing CI.

## What should happen

Two things, and the first is the architect's fault to fix rather than the
executor's to work around:

1. **Section E must preserve failure diagnostics.** A capture that can record
   a failure but not name it is not evidence. The block's section E is amended
   in § End-to-end verification to keep `... FAILED` lines and the `failures:`
   block alongside the summary.
2. **The intermittent failure must be understood, not out-waited.** Either
   root-cause it and make the test deterministic, or demonstrate with evidence
   that the failure mode is pre-existing and independent of this phase's six
   new tests. `STANDARDS.md` § 1 requires the test command to pass; a suite
   that passes 15 times in 16 does not.

Useful starting points, from the review's own investigation — none of them
conclusive, all of them cheap to check:

- The six new tests do real filesystem I/O under a process-global `HOME`
  swapped by `with_home`, which takes `crate::test_home_guard()`. Anything in
  the suite that mutates `HOME` **without** that guard can move
  `shell_logs_dir()` out from under a search that is mid-flight, which would
  make a shell's cast file unreadable, push it to `unsearchable`, and fail
  `find_in_shells_searches_only_scoped_shells`'s first assertion exactly the
  way a missing hit does.
- `TmpHome::new()` is `pid` + an atomic counter, so temp-HOME collisions
  between tests in one process are **not** the mechanism — checked at review.

## Root cause

Two, one on each side.

**Architect-side:** the E2E block's section E was written as
`cargo test --lib 2>&1 | grep '^test result'`. That is sufficient when every
run is green and useless the moment one is not — it throws away the only
information a failure carries. Every earlier phase in this milestone used the
same shape and got away with it because no run failed.

**Unknown, and that is the point:** the intermittent failure's mechanism is not
established. It involves a test this phase added, under full-suite parallelism,
at roughly 1-in-16. It did not reproduce in 10 runs at review.

## Definition of done

Each was run against the current tree (commit `0e661b92`) and returns the "now"
value shown. Doc greps are scoped — `<E2E>` is
`sed -n '/^## End-to-end verification/,/^## Authorizations/p' <doc> |` and
`<UL>` is `sed -n '/^## Update Log/,$p' <doc> |` — so neither this bug doc nor
the criteria block can satisfy them.

- [ ] **The capture preserves failure diagnostics:**
      `<E2E> grep -c 'FAILED'` → **1 or more** (now **0**). *(The amended
      section E is already in the phase doc; this criterion confirms it
      survived into the block the executor runs.)*
- [ ] **The filed section E is three clean runs:** in the newest
      `(end-to-end verification)` entry's pasted transcript,
      `<UL> sed -n '/^== E\./,/^== F\./p' | grep -cE '[1-9][0-9]* failed'` →
      **0** (now **1**), and the same range `grep -c '0 failed'` → **3 or
      more** (now **2**).
- [ ] **The flake is accounted for:** an Update Log entry headed
      `### Update — <date> (flake investigation)` exists —
      `<UL> grep -c '^### Update.*flake investigation'` → **1** (now **0**) —
      and it contains **either** a named root cause plus the change that fixes
      it, **or** evidence that the failure is independent of this phase's
      tests. In both cases it records **20 consecutive full-suite runs** with
      the amended section-E capture in place:
      `<UL> grep -c '^test result: ok. 1841 passed; 0 failed'` → **20 or more**
      (now **5**) — anchored to the line start so prose cannot satisfy it.
- [ ] **No behavioural regression:** `cargo test --lib 'knowledge::shell'` →
      **21**, unchanged; all 15 pre-existing test names still present
      (`grep -cE '    fn (list_shells|read_shell|formatter)' src/daemon/executor/knowledge/shell.rs`
      → **15**); `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **42**;
      `cargo test --test doc_truth` → **10**; all four gates pass.

## One claim in the completion summary to disregard

The run's `(complete)` summary states the flake is *"a pre-existing
suite-timing test, not a phase file — it does not reproduce"*. **Nothing
supports that attribution.** The capture discarded the failing test's name, so
the run had no basis for it; and the only test name that appears as `FAILED`
anywhere in the session logs is
`find_in_shells_searches_only_scoped_shells`, which **is** a phase file. The
"does not reproduce" half is fair — it did not, in 10 runs at review — but a
confident attribution with no evidence behind it is exactly what sends the next
reader looking in the wrong place. Treat the mechanism as **unknown** until the
flake-investigation entry establishes it.
