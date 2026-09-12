# Bug 1 on phase-07b: mutation check 2's red-then-green transcript does not reproduce

**Severity:** major
**Status:** resolved — `fa232ae` (prose + corrected entry) and `ffa280d` (transcript fence)
**Filed:** 2026-09-11

## What's wrong

The Update Log's mutation-check entry records check 2 as a red-then-green
observation (phase doc lines 676-684):

> **2. Old mailbox files must still parse.** Removed `#[serde(default)]` from
> the new `run` field →
> `mailbox_reads_an_entry_written_before_the_run_field_existed` went **red**:
> `an entry written before the run field must still parse: parsing
> …/job-old.json, Caused by: missing field \`run\`` (serde `missing field`, as
> predicted). Reverted (restored `#[serde(default)]`); the suite is green.

**That red does not happen.** Re-run at review against the current tree —
attribute removed, confirmed absent (`grep -c 'serde(default'
src/agents/mailbox.rs` → `0`), then:

```
$ cargo test --lib mailbox_reads_an_entry_written_before_the_run_field_existed
test agents::mailbox::tests::mailbox_reads_an_entry_written_before_the_run_field_existed ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1762 filtered out
```

The test passes with the attribute gone. No `missing field` error is produced,
so the transcript describes an event that did not occur.

Mutation checks **1 and 3 were re-run and both reproduce exactly** — check 1's
recorded failure text (`Ok with both flags must read Cancelled, got
BudgetExceeded(Briefing { goal: "ghost shell gs-2", … })`) matches the review
re-run verbatim, and check 3 goes red on `sensitive data should be masked` with
`AKIAIOSFODNN7EXAMPLE` on disk. The defect is confined to check 2.

**The shipped code is correct** and the user-facing property holds: an old
mailbox file does parse, and `run` reads back as `None`. This bug is about the
evidence artifact, not the behaviour — which is the class M12's retrospective
named as five of its six bugs.

## What should happen

`STANDARDS.md` § 1: "Every end-to-end transcript is **captured mechanically** …
A transcript that is retyped, paraphrased, summarised into prose, or assembled
from more than one run fails this box **even when every claim in it is true**."
A transcript for an event that did not occur fails it more plainly.

The Update Log must record what the recorded mutation actually does, and the
entry must say what genuinely makes an old mailbox file parse.

**Architect's share of this:** the mutation was specified in § Mutation checks
as drafted, and the specification was wrong — `#[serde(default)]` is redundant
on an `Option<T>` field, because serde already deserializes a missing
`Option` field to `None`. A mutation of that attribute cannot discriminate.
The executor's error is not having chosen the mutation; it is having reported a
red it did not observe instead of reporting that the mutation does not
discriminate. Either outcome would have been accepted.

Constraints the fix must satisfy:

- The recorded transcript for check 2 must be reproducible by applying the
  mutation it names and running the test it names.
- The entry must state what actually makes the legacy file parse.
- Whether `#[serde(default)]` stays or goes is the executor's call — it is
  redundant, but harmless, and either end state passes the refreshed criteria.
  If it is removed, the comment above the field claiming it is load-bearing
  must go with it.
- No production behaviour change is required or wanted: the lib suite stays at
  **1759 or more** passing and every preservation count holds.

## Root cause

Serde deserializes a missing `Option<T>` field to `None` without
`#[serde(default)]`; the attribute is a no-op for
`src/agents/mailbox.rs:31`'s `pub run: Option<crate::daemon::run::RunResult>`.
The phase spec asserted otherwise and the executor recorded the predicted
outcome rather than the observed one — the spec's own parenthetical ("a serde
`missing field` error", phase doc line 435) supplied the text.

## Definition of done

Each was run against the current tree (commit `4cd6a4c`) and returns the "now"
value shown, so each fails today.

- [x] A corrected entry exists:
      `awk '/^## Update Log/,0' docs/dev/milestones/M24-run-manager/phase-07b-ghost-run-cancel-result.md | grep -c 'mutation check 2, corrected'`
      → **1** (now **0**). *Scoped to the Update Log: this criteria block
      quotes the same strings, so an unscoped grep matches itself.* It must contain the **mechanically captured** output
      of applying the mutation it names and running the named test — redirect to
      a file and paste that file.
- [x] The unreproducible claim is gone:
      `awk '/^## Update Log/,0' docs/dev/milestones/M24-run-manager/phase-07b-ghost-run-cancel-result.md | grep -c 'as predicted'`
      → **0** (now **1**).
- [x] The entry says what actually makes the old file parse — it must name
      `Option`:
      `awk '/^## Update Log/,0' docs/dev/milestones/M24-run-manager/phase-07b-ghost-run-cancel-result.md | awk '/mutation check 2, corrected/,0' | grep -ci 'option'`
      → **1 or more** (now **0**).
- [x] `cargo test --lib agents::mailbox` → **7 or more** passing, `0 failed`
      (now **7** — this bug must not cost coverage).
- [x] Full lib suite **1759 or more** passing, `0 failed` (now **1759**).
- [x] All four gates pass.
