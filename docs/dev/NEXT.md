# NEXT

**Active phase: 10 — pane-prefs-redesign.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-10-pane-prefs-redesign.md`
Status: `todo` — drafted 2026-07-31, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-10`.

## What phase 10 does

Stops the pane preference silently targeting a pane the user never picked. The
mapping is `session_name → pane_id` and **both sides are unstable identities** —
after a tmux server restart `%0` almost certainly exists and is something else,
so the stored preference validates and the agent runs a foreground command in the
wrong pane with no prompt.

## The mechanism decision was made by the architect, not left open

The exit criterion says the mechanism is the phase's to choose. Given four
`NoProgressStall` hard-fails on this milestone — every one on open-ended
integration work — leaving a design decision open invites a fifth. So the phase
ships determinate: **fingerprint validation + pruning**, with the rationale and
the rejected alternatives recorded in both the phase doc and the milestone README.

**Open to your override at close.** The fallback is the scope reduction ("ask once
per daemon run"), which is strictly less work than what is specified, so nothing
is wasted if you prefer it.

## Three defects the phase fixes, all verified in the tree

1. **`pane_exists()` is not identity.** It proves *a* pane holds that ID, not that
   it is the pane the user chose.
2. **`get()` is implemented as `all.remove(session_name)`** — non-destructive only
   because the mutated map is never written back. One refactor from silently
   deleting every preference on read.
3. **Nothing prunes.** The live file still holds `de-phase01`, a long-dead
   rexyMCP session, and keys like `"0"`/`"1"`/`"2"` are tmux's default numeric
   session names, reused constantly — a preference stored for `"0"` is offered to
   any future session named `"0"`.

The phase also fixes `pane_prefs.rs`'s doc comment, which names
`~/.daemoneye/pane_prefs.json` while `prefs_path()` returns `var/run/`. That is
milestone defect 10, nominally phase 11's, but phase 10 rewrites that exact
comment — so it is folded in rather than having phase 11 reopen the file. The
orphaned file on disk is still phase 11's to remove.

## Where things stand

- Phases 01–09 `done`. **09 closed `escalated`** — architect takeover after the
  milestone's fourth `NoProgressStall`.
- 979 lib + 30 integration (2 ignored) + 8 isolation (1 ignored); clippy clean;
  `cargo test --lib` now clean across 12 consecutive runs (it was failing ~3 in 8
  before the phase-09 takeover fixed `HOME` poisoning).
- Working tree clean. No daemon running; no tmux server running.
- Milestone README:
  `docs/dev/milestones/M6-verification-and-hygiene/README.md`. Phases 11–12
  named, not drafted.

## Carried forward for milestone close

- **The stall pathology has shifted shape.** Phase 09's fourth
  `NoProgressStall` was a `sed -n 'N,N+10p'` paging crawl through a captured
  file — it evades the identical-call governor because every call differs, and
  only `read_only_stall_threshold` caught it. Four stalls now: phases 04, 06b, 08,
  09.


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
