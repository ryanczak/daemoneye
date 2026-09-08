# Bug 1 on phase-07b: two pinned acceptance criteria fail, and the Update Log says they pass

**Status:** open
**Severity:** major
**Filed:** 2026-09-07

## What's wrong

Two of the phase's own structural acceptance criteria are off-target against
the committed tree (`1e887fd`). Re-run of the phase's E2E block at review,
section F:

```
daemon: adopt_at_startup (1): 2
daemon: sweep_stale      (1): 2
```

The criteria pin both at exactly **1**:

> - [ ] **The registry is stood up and gated:** in `src/daemon/mod.rs`, …
>       `grep -c 'adopt_at_startup' src/daemon/mod.rs` → **1**,
>       `grep -c 'sweep_stale' src/daemon/mod.rs` → **1**, …

The pasted transcript is honest — it prints the `2`s. The `### Update —
2026-09-08 02:12 (end-to-end verification)` entry's prose is not:

> **F**: every structural grep prints its pinned number, including
> `spawn_detached unreachable (0): 0` … and `unwrap/expect/panic pre-test
> (0): 0` …

Two of the twelve greps do not print their pinned number. The completion
summary repeats the claim ("Day-5 DoD throughput: **17/17** — … F's twelve
greps"). A phase cannot self-report `complete` while acceptance criteria it
owns are failing; STANDARDS §1 requires *every* criterion verifiably met.

## What should happen

Both greps print `1`, and the E2E entry's prose reports what the transcript
actually shows.

The criteria mean **exactly one call site each** — that is what makes the
sweep-then-adopt sequence in `run_daemon` checkable by grep. The call sites
are correct and are not the problem; the second occurrence of each name is
prose inside a comment, which a `grep -c` cannot distinguish from a call.

## Root cause

The run's only substantive change to `src/daemon/mod.rs` was rewriting the
comment above the gated block (`src/daemon/mod.rs:735-739`), and the rewrite
introduced the function names into the comment text:

```rust
    // byte-for-byte as before (no sweep, no adopt, no directory creation:
    // `sweep_stale`/`adopt_at_startup` only read what is already there).
```

Before this run the comment named neither and both greps stood at `1` — the
`### Update — 2026-09-06 15:21 (escalation)` entry records "Section F: every
structural grep at its target value". So the run took two passing criteria to
failing and then asserted in prose that they passed.

This is the failure class `bug-07a-2` was filed for one phase earlier, in its
second form: there the transcript was paraphrased; here the transcript is
clean and the **prose summarising it** is what carries the false claim.

## Definition of done

- [ ] `grep -c 'adopt_at_startup' src/daemon/mod.rs` → **1** (now: `2`).
- [ ] `grep -c 'sweep_stale' src/daemon/mod.rs` → **1** (now: `2`).
- [ ] Exactly one call each remains: `grep -c 'crate::shell::adopt_at_startup'
      src/daemon/mod.rs` → **1** and `grep -c 'crate::shell::sweep_stale'
      src/daemon/mod.rs` → **1**.
- [ ] The E2E block is re-run and re-pasted into a new `(end-to-end
      verification)` Update Log entry whose section F shows
      `daemon: adopt_at_startup (1): 1` and `daemon: sweep_stale      (1): 1`,
      with the self-check printing `PASTE MATCH`.
- [ ] The new entry's prose states section F's result correctly; no claim that
      a grep printed a value it did not.
- [ ] All four gates pass.
