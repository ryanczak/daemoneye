# NEXT

**Active phase: 06b — webhook-to-ghost-e2e.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-06b-webhook-to-ghost-e2e.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-06b`.

## The blocker is resolved

The PE chose **option 3 (production observability seam) with option 1 (an
ignored full-daemon test) as a stopgap**. Phase 06b is drafted against that
decision.

**The seam:** `process_alert` returns `Option<JoinHandle<()>>` instead of
discarding the spawned ghost task's handle. Production behaviour is unchanged —
the HTTP handler still returns 200 without awaiting — but the handle now exists,
so a test can await it and assert `ghost_start` with no wall-clock waiting, and
production can finally answer "did that alert's ghost actually start?". The phase
explicitly forbids growing this into a task registry, cancellation, or stats
plumbing.

**The stopgap:** one `#[ignore]`d test drives the real HTTP path
(`start_daemon` → `post_webhook`), which cannot be deterministic because the
daemon spawns per alert, with §3.3's required justification in a comment.

## The hazard 06b must not trip

`start_session_with_config` calls `ensure_incident_session()`
(`src/tmux/session.rs:287`), which shells out to tmux and **creates a session
when none exists**. `Command` children inherit the parent environment, so an
in-process test that does not set `TMUX_TMPDIR` **will create a session on the
operator's live tmux server** — precisely M6 defect 13, the thing phase 01 was
built to prevent. The phase pins this and requires the test to assert the
default server is unchanged, using phase 01's `default_server_unchanged` as the
worked example.

## What the scenario proves

The chain `webhook_alert` → `webhook_analysis{ghost_trigger:true}` →
`ghost_start`, from a payload with **no severity field**. The middle assertion is
the load-bearing one: it proves phase 05's fail-open actually carried a
severity-less alert through the gate, which is the defect that motivated the
whole milestone.

The phase requires a mutation check on it — break the fail-open arm, confirm the
scenario fails, revert — because a scenario that passes while the pipeline is
broken is worth nothing.

## Where things stand

- Phases 01–06a `done`. 06a closed `approved_after_1`; its AI stub was
  mutation-verified twice (breaking the emitted token failed the test; moving the
  bind back inside the spawned task failed it 8/8), so 06b starts from a
  trustworthy instrument.
- `cargo clippy --all-targets --all-features -- -D warnings` clean; 964 lib + 30
  integration (2 ignored, pre-existing) + 7 isolation, zero failures.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 07–12
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
