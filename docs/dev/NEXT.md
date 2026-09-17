# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-09c-tmux-control-removal.md`
— 09c, tmux-control-removal. **Escalated 2026-09-17**, status `in-progress`.

The first run `hard_fail`ed with `VerifierFailurePersistent` (6 consecutive
failures, 41 turns) **while legitimately mid-batch**: removing the
`PendingCall::TmuxControl` variant makes the crate uncompilable until the last
arm is gone, and the verifier cannot tell "mid-batch" from "stuck".

**Lever chosen: resume in place.** The partial work is correct and is
**deliberately left uncommitted in the working tree** — it is the resume
substrate. Do not `git checkout`, `stash` or `reset` those four files:

```
 M src/ai/tools/args.rs      M src/ai/types/events.rs
 M src/ai/types/pending.rs   M src/daemon/stream.rs
```

Task 1 is complete (the variant, the event and the streaming arms are gone).
**Two compile errors remain and three edits close them** — `args.rs:100`,
`dispatch.rs:65`, `executor/mod.rs:1000`. The guidance is seeded as a
`Notes for executor` block at the top of the phase doc's Update Log, with the
ordering that reaches a green build fastest, which is exactly what the last run
could not do.

Resuming via `execute_phase` on the `in-progress` doc rather than
`continue_phase`: the latter is a blocking call with no `run_id` and no way to
stop it short of killing `rexymcp serve`.

Re-dispatch with `/rexymcp:dispatch phase-09c`.
