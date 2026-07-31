# NEXT

**Active phase: 09 — pane-and-archive-retention.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-09-pane-and-archive-retention.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-09`.

## What phase 09 does

Closes the last three artifact gaps phase 07's table left `Pending{phase-09}`:
a sweep for `var/log/panes/` (264 files, none today), a sweep for
`agents/*/mailbox/` (one file per ghost exit, forever), and **surfacing** the
asymmetry where `sessions.archive_retention_days` defaults to `0` (keep forever)
while `events.retention_days` defaults to `90`.

Both new retentions are **7 days** and both **must be operator-configurable** —
PE decision 2026-07-30, "we either do it now or we do it later". The phase says
outright that shipping the sweeps reading hard-coded constants is not acceptable.

## Two design calls made at drafting

**Surfacing is a startup WARN, not an IPC change.** The obvious operator-facing
surface is `daemoneye status`, but that payload is `Response::DaemonStatus` —
extending it touches `ipc.rs`, the server handler and `cli/status.rs` for a
one-line benefit. A startup WARN in `run_daemon` meets the criterion ("a sweep
that is off by default says so where the operator will see it") at a fraction of
the blast radius. The phase forbids the IPC route and says to report a blocker
instead of taking it.

**The warning is a pure function.** Following phase 08's split, which worked: a
function takes `&Config` and returns the warnings that apply; the daemon logs
them. The decision is testable; only the logging is a side effect.

**The default itself is untouched.** The criterion asks for visibility, not a
behaviour change — silently flipping a keep-forever default to a deleting one
would destroy operator data.

## Where things stand

- Phases 01–08 `done`. 08 closed `escalated` (architect takeover after the
  milestone's third `NoProgressStall`); the daemon-log rotation, its `dup2`
  re-attach, and the mutation-checked bound all landed.
- 972 lib + 30 integration (2 ignored) + 8 isolation (1 ignored), zero failures;
  clippy clean.
- Working tree clean. No daemon running; no tmux server running.
- **The NoProgressStall response fold landed** in `WORKFLOW.md` (§ "A
  NoProgressStall is usually a nearly-finished phase — diagnose the tree before
  choosing a lever"). If phase 09 stalls, run the gates against the partial tree
  before picking a lever; three-for-three the partial work was correct and the
  tree check found defects the executor never ran a gate to see.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 10–12
  named, not drafted.

## Carried forward for milestone close


- **Review subagents keep writing stray telemetry rows.** Five times this
  session a review agent probed the `rexymcp review` CLI with an invalid
  `--failure-class`, `--verdict`, or a fake `--phase-id`; the CLI records the row
  anyway with only a warning, and each had to be hand-deleted (backups at
  `~/.rexymcp/telemetry/phase_runs.jsonl.bak*`). One older stray from a prior
  session remains at line 314 (`phase_id: "x"`). Worth either rejecting invalid
  values outright rather than warning, or making probe/dry-run explicit.



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
