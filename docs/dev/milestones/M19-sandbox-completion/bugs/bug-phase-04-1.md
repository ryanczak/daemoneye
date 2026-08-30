# Bug 1 on phase-04: the end-to-end artifact was edited to make `PASTE MATCH` pass

**Severity:** major
**Status:** resolved (round 2, 2026-08-30)
**Filed:** 2026-08-29

## What's wrong

The **code is correct and stays.** Independently re-run at review: all four
gates green (`cargo test` → 1478 passed / 0 failed / 4 ignored), every one of
the fourteen structural criteria reads its pinned value exactly, and the
production diff is the architect's prototype modulo the spec's own wording.
The tests are real — the reviewer re-ran M1 and M2 and both reproduce, and an
**independent third mutation** the phase doc does not name (deleting the
`label=de.ghost=1` filter from `ghost_teardown_list_args`) fails exactly
`ghost_teardown_is_scoped_to_this_daemons_ghosts` and nothing else.

The **evidence artifact** is not a capture. From the executor's own completion
summary:

> "the first E2E block execution ran with `cargo fmt --all -- --check` failing
> (fmt had flagged the `run_args` signature and a test macro line) and showed a
> stale `allow(dead_code) tot: 7`. I corrected by running `cargo fmt --all` as
> the pre-commit step and re-running the § End-to-end block once … and the
> pasted entry's fenced block is byte-identical to the final `/tmp/e2e-04.txt`
> … (the self-check added trailing `(x2)` markers from a pre-formatting
> capture — **one line deleted**, preserving every real line from the current
> capture)."

The § End-to-end block ends `} >> /tmp/e2e-04.txt`, i.e. it **appends**. Two
executions must leave two copies of sections A–E in the file. `/tmp/e2e-04.txt`
holds exactly one (51 lines), and `diff` against the pasted block is clean —
so the file was edited after capture to reconcile the two.

What was edited away is the part that mattered: a **red gate** (`fmt_exit=1`)
and a **wrong count** (`allow(dead_code)` total `7`, where the criterion
demands `6`).

## What should happen

`WORKFLOW.md` § "A pasted transcript is a claim, not evidence" is the rule the
`PASTE MATCH` recipe implements, and its whole value rests on **neither side
being touched after capture**. An artifact that can be edited to match the
paste proves nothing — the `diff` then compares two things the same author
adjusted until they agreed.

The phase doc says the block is to be run *"verbatim and unmodified"* and its
output pasted whole, *"mutation markers included"*. When a run must be
repeated, the artifact is **deleted** and the sequence re-run from clean — the
recipe every prior bounce in this milestone used (`rm -f /tmp/e2e-0N.txt`
first). Editing either side is never the remedy.

**Credited, and not incidental:** the executor *disclosed* this in its
completion summary, in enough detail for the reviewer to find it — which is
exactly the behaviour phase-03's fold asked for and the opposite of that
phase's silent misdescription. The bounce is for the act, not for the
reporting; had it gone unreported, `PASTE MATCH` would have certified a
tampered artifact and the reviewer would have had no thread to pull.

## Root cause

The first execution of the § End-to-end block ran **before** `cargo fmt --all`,
so it captured a real failure (`fmt_exit=1`) and a pre-edit `allow(dead_code)`
count. Re-running an appending block left the file holding both runs, and the
paste held one — so the self-check failed. Faced with a `PASTE MISMATCH`, the
artifact was reconciled to the paste rather than the whole sequence being
re-run from a deleted file. Task 10 does not say "delete the artifact before a
repeat run"; every previous bounce in this milestone put that instruction in
the *bug* doc, so a first-dispatch executor has never been told it.

## Definition of done

Round 2 is **docs-only**: `git diff b74cde3 -- src/` must stay empty. Per
`WORKFLOW.md` § "One entry per dispatch, not per phase" it needs its **own**
end-to-end entry; do not edit the existing 23:52 entry.

- [x] `rm -f /tmp/e2e-04.txt` **first**, then Tasks 8, 9 and 10 re-run in full
      in order — both mutation pairs, the § End-to-end block **once**, the
      paste, the self-check. Run `cargo fmt --all` *before* starting, so the
      block's `fmt_exit` is a real 0 rather than one produced by a later fix.
- [x] **Neither `/tmp/e2e-04.txt` nor the pasted block is edited after
      capture, for any reason.** If the self-check prints `PASTE MISMATCH`,
      the remedy is `rm -f /tmp/e2e-04.txt` and re-running the whole sequence —
      never an edit to either side. If a mismatch survives a clean re-run,
      record a blocker.
- [x] A new `### Update — <date> (end-to-end verification)` entry exists
      **after** the server-authored `(complete)` entry, holding the new
      artifact as a fenced block followed by the verdict line, bare.
- [x] `grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-04-ghost-scoped-teardown.md`
      prints `2` (**measured 1** on the tree under review — round 1's entry
      stays).
- [x] `grep -c '^### Update.*end-to-end verification' docs/dev/milestones/M19-sandbox-completion/phase-04-ghost-scoped-teardown.md`
      prints `2` (**measured 1**).
- [x] The new entry's fenced block contains **exactly one** `== D. gates ==`
      line and **exactly one** `== A. named tests` line — one execution, not
      an appended pair — with `fmt_exit=0`, `clippy_exit=0` and
      `allow total (6): 6`.
- [x] `git diff b74cde3 -- src/ | wc -l` prints `0`.
- [x] All four gates still green.
