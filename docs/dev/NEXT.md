# NEXT

**Active phase: 07 — artifact-lifecycle-policy.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-07-artifact-lifecycle-policy.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-07`.

## M6's headline verification now exists and passes

Phase 06b landed `approved_first_try`. A payload with **no severity field** now
provably reaches a ghost shell, observable end-to-end. The real record, captured
from the event log during the run:

```json
{"alert_name": "disk-full", "event": "ghost_start",
 "session_id": "ghost-disk-full-a2123d98…", "tmux_session": "daemoneye-incidents",
 "spawn_depth": 0, "trigger": "de-gs-bg-"}
```

That is defect 1 — the milestone's motivating bug — demonstrated rather than
argued. The review reproduced the mutation check independently: breaking the
fail-open arm makes the scenario fail with `process_alert returned None — no
ghost spawned`.

**The PE-approved seam shipped with it.** `process_alert` returns
`Option<JoinHandle<()>>` instead of discarding the spawned ghost's handle;
production behaviour is unchanged (`server.rs:86` still returns 200 without
awaiting), but the spawn is now observable. `maybe_analyze_alert`'s signature
changed to propagate it — the review judged that in-spec.

## What phase 07 does

States, in one place, what happens to **every** artifact class under
`~/.daemoneye/`, and lands the test that fails when a class exists with no stated
policy. It writes **no rotation code** — phases 08 and 09 implement against the
table.

## The design decision this phase turns on

The exit criterion says *"no class is unmanaged by omission"* — **omission** is
the sin, not unmanagement. So the table records an *intended* lifecycle (rotate /
delete / archive / keep-forever), its default, **and whether that intent is
implemented yet, with the owning phase**. `daemon.log`'s policy is "rotate — not
yet implemented, phase 08". That is a stated policy; recording nothing is not.
Without this distinction the test would fail on day one for three classes and
tempt whoever hits it into writing rotation code here, producing exactly the
fourth independent convention the 07→08→09 ordering exists to prevent.

The phase is the structural sibling of phase 02: an explicit table plus a test
checked in **both** directions (no class escapes the policy; no entry is
fiction). It points the executor at `src/config/path_audit.rs` as the pattern,
which it has now succeeded with twice.

## Where things stand

- Phases 01–06b `done`. 964 lib + 30 integration (2 ignored) + 8 isolation
  (1 ignored, the full-daemon HTTP stopgap), zero failures.
- `cargo clippy --all-targets --all-features -- -D warnings` clean.
- Working tree clean. No daemon running; no tmux server running. An orphaned
  private tmux server left by an early 06b run was terminated; the operator's
  default server was never touched.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 08–12
  named, not drafted. Re-verify each phase's "Current state" against the tree
  before dispatching.

## Carried forward for milestone close



- **A pre-existing `tokio::time::sleep` at `tests/integration.rs:615`** violates
  `STANDARDS.md` §3.3. It predates M6 and sits outside every phase's
  Authorizations, so two reviews correctly declined to touch it. Worth a decision
  alongside the §3.3 question above.

- **A second E2E-transcript fold is worth considering.** The first fold
  (`STANDARDS.md` §1 capture box + `WORKFLOW.md` step 4) is working — it caught
  bug-04-4, where a real `diff` prints nothing on identical input so
  `"(empty - no changes)"` could only have been hand-typed. But the requirement
  still failed on phase 05, and the likely structural cause is that the
  **server-authored `(complete)` entry** carries a "Command output tails" block
  that looks like captured evidence while being the standard gate capture every
  phase gets. Naming that explicitly in the refinement unblocked the executor
  immediately. Candidate fold: the E2E block must be a distinct executor-authored
  entry, and the server-authored gate tails do not satisfy it.

- **`.gitignore` has no `.daemoneye/` entry.** A full seeded 168K runtime tree
  was found untracked in the repo root during phase 04 and had to be moved out
  before it was committed. Two reviews recommended the entry; both correctly
  declined to make it, as it sits outside any phase's Authorizations. It is
  milestone housekeeping.
- **`src/main.rs:17` and `:30`** still document the daemon log as
  `~/.daemoneye/daemon.log`; the real path is `var/log/daemon.log`. Same drift
  class as the prompt defect, in CLI help text the asset gate does not cover.
  Noted for phase 11.
- **The dedup log-level narrowing** described above.
