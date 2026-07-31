# NEXT

**Active phase: 12 — roadmap-correction. This is M6's LAST in-scope phase.**
Doc: `docs/dev/milestones/M6-verification-and-hygiene/phase-12-roadmap-correction.md`
Status: `todo` — drafted 2026-07-31, not yet dispatched.

Dispatch with `/rexymcp:dispatch phase-12`.

**When phase 12 is approved the loop stops at the milestone boundary** — an
absolute human gate. The retrospective, any calibration folds, and the go/no-go
for M7 are all yours.

## What phase 12 does

Two things, plus one deliberate omission.

1. **Brings `docs/architecture.md` § 5 up to date.** The exit criterion said it
   "no longer points at a superseded active milestone" — that was already fixed
   when M6 was scoped, so § 5 correctly reads `Active milestone — M6`. What is
   stale is M6's *own* entry: it claims the README has "twelve phases named, none
   drafted". All twelve are drafted, eleven are `done`, and 06 was split into
   06a/06b, so there are thirteen docs.
2. **Removes a false belief the agent is still being told.** Found while
   drafting: `agent-runtime-layout.md:40` describes `memory/var/index/memory.db`
   as an FTS5 SQLite index. `src/memory/index.rs` is an eight-line stub returning
   empty and nothing in `src/` references `memory.db` or `var/index` — the file
   never exists. That is defect class 4/5 exactly, and it survived phases 02 and
   03 only because it lives in a **code fence**, which the extractor is blind to
   by design.

**Deliberately not done:** the phase does **not** relabel M6 as shipped and does
**not** write the retrospective. Phase 12 makes § 5 true; the close makes it
final, and that is your call.

## One thing verified rather than assumed

§ 5's note that the FTS5 index is "currently a stub" is **still accurate** —
`src/memory/index.rs` really is eight lines returning `Vec::new()`. The phase
leaves that note alone; it is the doc being honest, and the agent-facing asset is
what was wrong.

## Where things stand

- Phases 01–11 `done`. **11 closed `escalated`** — architect takeover for the
  end-to-end capture only; its `HOME`-flake fix was the executor's and is
  verified at 0-in-16.
- 990 lib + 30 integration (2 ignored) + 8 isolation (1 ignored); clippy clean.
- Working tree clean. No daemon running; no tmux server running.
- `~/.daemoneye/lib/` and `~/.daemoneye/pane_prefs.json` are still on disk by
  design — the code no longer creates or reads either. Remove when convenient:
  `rmdir ~/.daemoneye/lib && rm ~/.daemoneye/pane_prefs.json`

## Carried forward for milestone close

- **`CLAUDE.md` over-describes the memory index.** It calls
  `src/memory/index.rs` an "FTS5 index (`var/index/memory.db`): schema,
  reconciliation, CRUD, BM25 search with grep fallback (G1); G2
  `migrate_schema()`". The file is an eight-line stub. `CLAUDE.md` is
  developer-facing rather than agent-facing so it sits outside the path-audit
  gate, but it is the same drift class and shapes how work gets planned.
- **~25 test files set `HOME` without restoring it**, of which only about five
  do. Phase 11 fixed the four in `lifecycle.rs` and pinned the one victim in
  `path_audit.rs`, which closed the observable flake — but the drift remains and
  will resurface the next time an ambient reader is added. A cleanup phase of its
  own.

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
