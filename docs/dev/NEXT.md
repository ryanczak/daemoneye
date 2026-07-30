# NEXT

**Active phase: 08 — daemon-log-rotation.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-08-daemon-log-rotation.md`
Status: `todo` — drafted 2026-07-30, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-08`.

## What phase 08 does

Bounds `var/log/daemon.log` — 25.8 MB and growing since May 8, with no rotation
logic anywhere in the tree. Phase 07 recorded its policy as **Rotate**, owned by
phase 08; this phase makes that true and flips the table entry to implemented.

## The constraint that decides the design

**The log is not written through a Rust writer.** `run_daemon`
(`src/daemon/mod.rs:371-394`) opens the file `O_APPEND` and `dup2`s its
descriptor onto **stdout (1) and stderr (2)**; `env_logger` then writes to
stderr.

So a plain `rename(daemon.log, daemon.log.1)` **is not rotation** — fds 1 and 2
still point at the same inode, now renamed, so logging would silently continue
into the rotated file while the live log stayed empty. That is a rotation that
looks like it worked and didn't. Whatever lands must re-open the new path and
`dup2` the fresh descriptor onto 1 and 2 (or truncate in place, which `O_APPEND`
makes safe).

The phase pins a **testability seam** around that: file-shifting is a pure
function taking path, size bound and keep-count as parameters, callable straight
from a test; the `dup2` re-attach is process-global and stays in the daemon path.
A rotation function that does its own `dup2` internally cannot be tested, and the
phase says so explicitly.

It also reuses the existing cleanup tick (`src/daemon/mod.rs:819-828`, every 60th
iteration, already firing the two sweeps) rather than adding a timer — and notes
that a startup-only check would not bound a daemon that runs for weeks, which is
exactly how the live log got to 25.8 MB.

## Where things stand

- Phases 01–07 `done`. 07 closed `approved_after_1`; the reviewer independently
  reproduced its mutation (injected an uncovered directory → genuine failure
  naming it at `exit=101` → revert → 3/3 pass).
- 967 lib + 30 integration (2 ignored) + 8 isolation (1 ignored), zero failures;
  clippy clean.
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 09–12
  named, not drafted.

## Needs your attention before phase 09

**Phase 07's table carries two invented retention numbers** — `var/log/panes` →
`Sweep{30 days}` and `agents/*/mailbox` → `Sweep{7 days}` — for classes that have
**no config key today**. The executor chose them; the phase doc asked for "the
default where the lifecycle is parameterised" without supplying numbers. Both
reviews judged them acceptable as explicitly-labelled `Pending{phase-09}`
proposals that cannot silently take effect, but round 1 raised the sharper point:
the table renders a proposed number identically to a real config-backed one
(`Sweep{30}` looks exactly like `Sweep{90}`). **Retention periods are an
operational decision** — worth setting yourself before phase 09 implements
against them.

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
