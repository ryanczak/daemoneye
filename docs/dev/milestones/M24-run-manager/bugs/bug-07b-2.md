# Bug 2 on phase-07b: the fabricated mutation-2 red survives in the fenced transcript

**Severity:** major
**Status:** open
**Filed:** 2026-09-11

## What's wrong

bug-07b-1 was fixed in the **prose** and in a new corrected entry, but not in
the evidence block. The `### Update — 2026-09-11 16:56 (mutation checks, red →
green)` entry ends with a fenced block headed "Full transcript below", and that
block still reads (phase doc lines 734-737):

```
(2) red: cargo test --lib mailbox_reads_an_entry_written_before_the_run_field_existed
  an entry written before the run field must still parse: parsing .../job-old.json,
  Caused by: missing field `run`
(2) green after revert: agents::mailbox test result: ok. 7 passed; 0 failed
```

That red still does not happen. Re-verified at this review with the attribute
genuinely removed (`grep -c 'serde(default' src/agents/mailbox.rs` → `0`):

```
test agents::mailbox::tests::mailbox_reads_an_entry_written_before_the_run_field_existed ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1762 filtered out
```

The document now contradicts itself: the prose 20 lines above (line 712) says
the mutation does not discriminate and the red does not reproduce, while the
block presented as the captured transcript still shows it.

**What the re-dispatch got right, and is not being asked to redo:** the
corrected entry at line 922 is accurate — I applied its mutation and got its
stated result, `ok. 1 passed`. The prose correction is right. Mutation checks 1
and 3 remain sound. The code is untouched and correct: gates green, lib suite
**1759 passed / 0 failed**, `agents::mailbox` **7 passed**.

## What should happen

`STANDARDS.md` § 1: a transcript that is "retyped, paraphrased, summarised into
prose, or assembled from more than one run fails this box **even when every
claim in it is true**." One describing a run that never happened fails it
outright, and it fails the same whether it sits in prose or in a fence.

The fenced block must stop presenting an unreproducible red as captured output.
Either form is acceptable:

- delete the three `(2)` lines from the block, leaving checks 1 and 3 (both
  reproduce); or
- replace them with an explicit retraction that does not read as transcript
  output — e.g. a line saying the check was withdrawn and pointing at the
  corrected entry.

Do not re-run checks 1 or 3, do not touch code, and do not add a third Update
Log entry about this — amend the block in place.

**Architect's share of this:** bug-07b-1's Definition of Done pinned the prose
string (`as predicted` → 0) and never pinned the fenced block, so a fix that
satisfied every stated criterion could still leave the artifact. That is an
under-specified DoD, and it is the second time in this phase a criterion of
mine matched less than it was meant to police. The criterion below is scoped to
the exact entry.

## Root cause

The correction targeted the narrative and appended a new entry; the fenced
block at the end of the 16:56 entry was a separate copy of the same claim and
was left alone. Nothing in bug-07b-1's DoD referenced it.

## Definition of done

Each was run against the current tree (commit `16f56e6`) and returns the "now"
value shown, so each fails today.

- [ ] **The fabricated red is gone from the mutation-checks entry:**
      `awk '/^### Update — 2026-09-11 16:56/,/^### Update — 2026-09-11 16:57/' docs/dev/milestones/M24-run-manager/phase-07b-ghost-run-cancel-result.md | grep -c 'Caused by: missing field'`
      → **0** (now **1**). *Scoped to that entry: the corrected entry and these
      criteria both legitimately discuss the string.*
- [ ] **Checks 1 and 3 keep their transcripts** — same `awk` range:
      `grep -c '^(1) red:'` → **1** and `grep -c '^(3) red:'` → **1**, both
      unchanged (now **1** each).
- [ ] `cargo test --lib agents::mailbox` → **7 or more** passing, `0 failed`
      (now **7**).
- [ ] Full lib suite **1759 or more** passing, `0 failed` (now **1759**).
- [ ] **No code change:**
      `git diff 16f56e6..HEAD --stat -- src/ tests/ | wc -l` → **0**.
- [ ] All four gates pass.
