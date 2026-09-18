# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-09d-pane-param-retirement.md`
— 09d, pane-param-retirement. **Escalated 2026-09-17**, status `in-progress`.

The first run `hard_fail`ed with `VerifierFailurePersistent` (6 consecutive
failures, 76 turns) **while legitimately mid-refactor** — the same structural
interaction 09c hit, and resume cleared it there.

**Lever chosen: resume in place.** The partial work is correct and is
**deliberately left uncommitted in the working tree** (9 source files). Do not
`git checkout`, `stash` or `reset` them.

Already done and verified: `FgArgs.target` removed, `run_background`'s
`retry_pane` parameter removed, and `defs.rs` / `args.rs` / `pending.rs` /
`events.rs` / `stream.rs` / `ghost.rs` / `gemini.rs` all edited.

**`cargo build` reports 12 errors but there are only FIVE edit sites** — six of
the twelve are cascade `E0277` trait-bound noise emitted at the same two call
sites as the `E0061` arity errors, and they vanish when the arity is fixed. The
guidance block at the top of the phase doc's Update Log names all five with
line numbers.

It also warns off a trap: **rustc offers a machine-applicable suggestion here
that is wrong** — "restrict type parameter `W` with `AsyncBufRead`". `W` is the
write half; applying it would be invasive and incorrect.

Re-dispatch with `/rexymcp:dispatch phase-09d`.
