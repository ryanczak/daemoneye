# Bug 3 on phase-08e: no test pins the answer-once window — Mutation A survives

**Severity:** minor
**Status:** closed 2026-09-29 — verified fixed at review. Test 3's fake now sleeps 0.3 s after the answer; the reviewer re-applied Mutation A and it went red 2/2, and the unmutated tree is green.
**Filed:** 2026-09-28

## What's wrong

The phase's Mutation A (classify `&buf` instead of `&buf[answered_at..]`) was
predicted red on test 3; the executor ran it and honestly recorded **green**.
Its diagnosis is right: every fake touches its flag and exits immediately
after the right answer, so the next loop turn takes the child-exit branch
before the stale prompt can be re-classified. The 08f rule — answer only a
prompt that appeared after the last answer — is therefore unpinned by any
test in this phase, while real ssh spends tens of ms to seconds
authenticating after the answer (the window where a re-classified stale
prompt would trigger a second `CredentialPrompt`).

(The mispredicted mutation is an architect-side defect; the executor's
observation stands. The fix is still a test change.)

## What should happen

A test goes red under Mutation A.

**Measured by the reviewer:** adding `sleep 0.3;` after the first
`read -r pw;` in test 3's script (`a_password_prompt_is_answered_once_and_the_host_connects`,
`src/daemon/executor/shell_exec.rs:2916`) — modelling ssh's post-answer auth
latency — gives: unmutated tree **green 3/3**; with Mutation A applied
**red 3/3** (`Err("a password prompt appeared and nobody is attached …")`,
the second round trip hitting the exhausted transport).

## Root cause

The fakes collapse "answer accepted" and "process exited" into one instant,
so the exit branch masks the classification window.

## Definition of done

- [ ] Test 3's fake (or a new test in `connect_tests`) keeps the child alive
      for a measurable interval after the correct answer; the test passes on
      the current code.
- [ ] Mutation A re-applied (via the `patch` tool) turns it **red**; observed
      result recorded in the Update Log; restored, `git diff --stat` back to
      pre-mutation.
