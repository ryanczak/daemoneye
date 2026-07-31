# NEXT

**Active phase: 11 — runtime-tree-hygiene.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-11-runtime-tree-hygiene.md`
Status: `todo` — drafted 2026-07-31, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-11`.

## What phase 11 does

Makes `~/.daemoneye/` contain nothing the code does not deliberately produce, and
nothing the docs describe that the code does not create. Three concrete items plus
a gate:

1. **`lib/` — decided: drop it.** Created on every install, empty since 26 March
   in the only live tree available, documenting `de_sdk`/Python helpers that exist
   nowhere in the repo. The phase removes it from `ensure_dirs()`, the path-audit
   inventory, the lifecycle table and the knowledge-memory asset.
2. **The CLI help strings** at `src/main.rs:17` and `:30` still name
   `~/.daemoneye/daemon.log`; the real path is `var/log/daemon.log`. Same drift
   class phase 03 fixed in the assets — but the phase-02 gate only audits assets,
   so CLI help was never covered.
3. **`.gitignore` gets a `.daemoneye/` entry.** During phase 04 a full 168 KB
   seeded tree appeared untracked in the repo root and had to be moved out before
   a `git add -A` swept it in. Two reviews recommended this; both correctly
   declined as out of scope.

Plus the durable part: a test asserting the directories `ensure_dirs()` creates
are exactly the set the policy table documents — Direction A already covers "no
directory without an entry"; the missing half is "no non-lazy entry without a
directory", which is precisely `lib`-shaped drift.

## The interlock worth watching

Removing `lib` from the path-audit inventory makes any surviving `lib/` mention
in an audited asset an `Unknown` finding, turning the phase-02 gate red. So a
partial job fails loudly rather than silently — the gates built earlier in this
milestone now enforce this phase's completeness.

## Two things left for you deliberately

- **`~/.daemoneye/pane_prefs.json`** (12 bytes, 25 June) is dead —
  `pane_prefs::prefs_path()` returns `var/run/pane_prefs.json`. The phase does
  **not** delete it, because it lives in your real tree and this milestone has
  been careful about code that removes user data. Remove it when convenient:
  `rm ~/.daemoneye/pane_prefs.json`
- **`~/.daemoneye/lib/`** likewise stays on disk; the phase only stops creating
  it. `rmdir ~/.daemoneye/lib` once you are satisfied.

## Where things stand

- Phases 01–10 `done`. 10 closed `approved_after_2` after two bounces; its
  non-mutation test now asserts mtime as well as content, so a byte-identical
  write-back is caught.
- 989 lib + 30 integration (2 ignored) + 8 isolation (1 ignored); clippy clean;
  twelve consecutive `cargo test --lib` runs clean.
- Working tree clean. No daemon running; no tmux server running.
- **Only phase 12 remains after this** — `docs/architecture.md` § 5 still names
  M4 as the active milestone.

## Carried forward for milestone close

- **A spurious `boundary` activity row for phase-10** was journaled when a review
  subagent was interrupted by accident. It is an activity record, not a verdict,
  so it does not affect the scorecard — left in place rather than hand-editing the
  telemetry store again.

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
