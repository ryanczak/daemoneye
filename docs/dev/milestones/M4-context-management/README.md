# M4 — Context Management Overhaul

**Goal:** Make DaemonEye's conversation-context management survive very long
daemon uptimes (hundreds of days) and very long chat sessions (thousands of
turns): per-turn cost bounded regardless of session age, no irreversible
context loss, compaction off the interactive hot path.

**Status:** complete (2026-07-16)

**Depends on:** M3 (Polish & Maintenance) — complete.

**Design:** [`docs/design/context-management.md`](../../../design/context-management.md)
— the failure catalog (D1–D15) and target architecture every phase references.

**Exit criteria:** — all met (2026-07-16).

- [x] `events.jsonl` is rotated into dated segments with retention; no reader
      loads the whole event history into memory (D4). — phase 01.
- [x] Compaction is token-budgeted with hysteresis: a compaction pass frees
      at least `compact_at_pct − target_pct` of the context window, and a
      session held at high token pressure does not re-digest every turn (D8).
      — phase 03.
- [x] No code path rewrites or truncates a session archive file; every
      message ever exchanged in a session is recoverable from
      `<id>.archive.jsonl` until retention deletes the file (D1). — phase 04.
- [x] Dropped context is model-recoverable: the `recall_context` tool returns
      archived turns by query or turn range, and elision/epoch text names the
      tool instead of claiming the data is in `events.jsonl` (D2). — phase 07.
- [x] Compacted history is represented as an append-only epoch chain with
      per-span tallies and chapter rollups — the in-context representation of
      a 1000+-turn session is O(log turns), not a single 15-line summary
      (D3, D5). — phases 05a/05b/06.
- [x] The narrative/tally epoch build runs off the interactive path; a user
      turn is never blocked on the summarizer model except via the >= 85%
      emergency path (D11). — phase 08.
- [x] Daemon restart or 30-minute eviction preserves `started_at`,
      `turn_count`, token calibration, and reloads history at a clean turn
      boundary (D10). — phases 02/09.
- [x] Ghost sessions get elision + structured compaction (D13). — phase 10a.
- [x] `cargo clippy --all-targets --all-features -- -D warnings` stays clean;
      no `.unwrap()` on locks (`.unwrap_or_log()` per `src/util.rs`).

## Architecture references

- `docs/design/context-management.md` — the M4 design (primary reference).
- `docs/architecture.md#12-orchestration-layer-srcdaemon` — where the
  compaction path lives.
- `docs/architecture.md#21-interactive-requestresponse` — the request
  lifecycle the async-compaction phase must not stall.
- `docs/architecture.md#3-the-ghost-shell-subsystem` — ghost turn loop
  (phase 10).

## Scope notes

- **Drafted ahead by explicit PE request (2026-07-07).** All ten phase docs
  were authored at milestone kick-off, deviating from the expand-on-demand
  default in WORKFLOW.md. Consequence: the **Current state** section of any
  phase ≥ 02 may be stale by dispatch time (earlier phases move its anchors).
  Each phase doc carries a Pre-flight step requiring re-verification of its
  Current state against the working tree; the architect re-validates the doc
  before `/rexymcp:dispatch`.
- **No new dependencies in M4.** `recall_context` uses the `search.rs` grep
  idiom; FTS5 (rusqlite) and zstd compression are future work
  (design doc §8). Any phase that appears to need a dependency is a blocker,
  not an authorization.
- **Rows may be re-split** if a phase exceeds one executor session
  (~500 lines of diff), per WORKFLOW.md.
- **Task shape:** phases 01, 02, 04, 09 are mechanical-to-moderate (normal
  spec density); phases 03, 05, 06, 07, 08, 10 hide design decisions and get
  front-loaded constraint paragraphs + worked examples (M2 calibration fold).

## Phases

| #  | Phase | Theme | Status |
|----|-------|-------|--------|
| 01 | events-rotation ([phase-01-events-rotation.md](phase-01-events-rotation.md)) — dated event segments, shared streaming readers, retention sweep | hygiene | done |
| 02 | token-estimation ([phase-02-token-estimation.md](phase-02-token-estimation.md)) — `context/estimate.rs`, per-session calibration, restart blind-spot fix | signal | done |
| 03 | budget-compaction ([phase-03-budget-compaction.md](phase-03-budget-compaction.md)) — `[compaction]` config, token-budget cut + hysteresis, synthesized boundaries, `[BUDGET]` rewording | core | done |
| 04 | append-only-archive ([phase-04-append-only-archive.md](phase-04-append-only-archive.md)) — `<id>.archive.jsonl`, honest elision placeholders, retention | core | done |
| 05a | epoch-persistence ([phase-05a-epoch-persistence.md](phase-05a-epoch-persistence.md)) — `context/epochs.rs` types, append-only persistence, span-windowed tally/scan (additive) | core | done |
| 05b | epoch-head ([phase-05b-epoch-head.md](phase-05b-epoch-head.md)) — `compact_with_epochs` regenerated head, keep-newest narrative, retire the digest path | core | done |
| 06 | ledger-rollups ([phase-06-ledger-rollups.md](phase-06-ledger-rollups.md)) — session ledger + chapter rollups (O(log n) representation) | core | done |
| 07 | recall-context ([phase-07-recall-context.md](phase-07-recall-context.md)) — the `recall_context` AI tool over the archive | core | done |
| 08 | async-compaction ([phase-08-async-compaction.md](phase-08-async-compaction.md)) — background epoch build, staleness-checked swap, emergency path | core | done |
| 09 | session-meta-persistence ([phase-09-session-meta-persistence.md](phase-09-session-meta-persistence.md)) — `<id>.meta.json`, boundary-safe reload | hygiene | done |
| 10a | ghost-coverage ([phase-10a-ghost-coverage.md](phase-10a-ghost-coverage.md)) — synchronous model-call-free ghost working-set guard (D13) | coverage | done |
| 10b | memory-extraction ([phase-10b-memory-extraction.md](phase-10b-memory-extraction.md)) — opt-in compaction→memory fact extraction | coverage | done |

## Notes

### Retrospective (2026-07-16)

**Outcome.** All ten phases (twelve docs after two mid-milestone splits) reached
`done`; every M4 exit criterion is met. Final gates green: `cargo test` = 901
lib-unit + 27 integration passing, 0 failures (2 ignored); clippy
`--all-targets --all-features -- -D warnings` clean. Test count grew 862 → 901
lib-unit across the milestone. Span: 2026-07-07 (scoping) → 2026-07-16 (phase-10b
close), ~44 commits.

**What shipped.** The compaction path was rebuilt end to end: dated event
segments with retention + streaming readers (01); deterministic token estimation
with per-session EMA calibration and a restart blind-spot fix (02); token-budgeted
compaction with hysteresis and synthesized boundaries (03); an append-only
`<id>.archive.jsonl` that no path ever rewrites (04); an append-only epoch chain
with per-span tallies (05a/05b) and O(log n) chapter rollups (06); the
`recall_context` tool over the archive (07); background async epoch build with a
staleness-checked swap and a ≥85% emergency path (08); `<id>.meta.json` continuity
across restart/eviction (09); a synchronous model-call-free ghost working-set
guard (10a); and opt-in compaction→memory fact extraction (10b).

**Verdicts.**

| Phase | Verdict | Executor path |
|---|---|---|
| 01 events-rotation | done (1 bounce) | session takeover (verify-loop on bug re-check) |
| 02 token-estimation | approved_after_1 | executor (bug-02-1: no-op blind-spot fix) |
| 03 budget-compaction | done | architect takeover (git-thrash, 352 turns) |
| 04 append-only-archive | approved_first_try | executor (pure additive) |
| 05a epoch-persistence | done | minimal takeover (governor-caught verify-loop, 6 calls) |
| 05b epoch-head | done | architect takeover (529-turn verify-loop, human-stopped) |
| 06 ledger-rollups | done | architect takeover (167-turn verify-loop, human-stopped) |
| 07 recall-context | done | architect takeover (2 no-progress stalls, governor-caught) |
| 08 async-compaction | done | architect takeover (2 `NoProgressStall` hard_fails) |
| 09 session-meta-persistence | approved_after_1 | executor resume + 1 review bounce (bug-09-1) |
| 10a ghost-coverage | approved_first_try | executor (additive; first epoch-path clean run) |
| 10b memory-extraction | done | architect takeover (`LowNoveltyStall` on +309-line block) |

**The dominant pattern — confirmed, not new.** WORKFLOW.md § "Executor
self-sabotage on delete-heavy rewrites is a runtime concern" (folded mid-milestone
from phases 01/03/05a/05b) predicted the rest of the milestone exactly. **Every
compaction/epoch-path phase that rewired a load-bearing path needed architect
takeover** — 03, 05a, 05b, 06, 07, 08, 10b. The only clean runs were the two
purely additive phases, 04 and 10a (`approved_first_try`), plus the two mechanical
signal/hygiene phases 02 and 09 (`approved_after_1`). The task-shape thesis held:
additive shape → executor succeeds; delete-and-rewire shape → executor thrashes or
verify-loops.

**Mitigations that worked.** (1) The 05a/05b and 10a/10b **additive-vs-rewire
splits** contained blast radius every time — the additive half landed clean and
survived on disk while the rewire half was salvaged by takeover; 10a was the first
epoch-path phase to reach `done` with no takeover at all. (2) **Takeover over
resume** when correct code was already on disk (resume just re-enters the loop) —
proven repeatedly. Both are already in the WORKFLOW fold.

**One genuinely new variant — candidate fold (held for sign-off).** Phase-10b was
a *pure large addition* (+309 lines), not a delete-heavy rewrite, yet it still
self-sabotaged — the executor corrupted adjacent existing code while inserting the
block, and stalled on `LowNoveltyStall`. The existing fold is scoped to
*delete-heavy* rewrites; a large *additive* block into an existing file is a
distinct trigger the current fold text does not name. This is the milestone's one
new data point and the candidate extension to the fold (see § Calibration below).

**rexyMCP runtime co-evolution.** M4 drove four rexyMCP governor changes, all
landed mid-milestone: FR-1 (`git stash`/`checkout HEAD --` hard-block, `a9399a0`),
FR-2 (no-progress read-only stall detector, `2a405a7`), the rexyMCP#2 governor fix
(unblocked phase-09's resume), and rexyMCP#3 (`LowNoveltyStall` novelty-aware
detection, validated in the wild on 10b). The governors demonstrably shortened
runaways: phase-05b's pre-governor 529-turn loop became phase-07's governor-caught
20/40-turn stalls.

**Calibration.** No new WORKFLOW.md fold landed this milestone without sign-off.
One candidate (large-additive self-sabotage) is surfaced below for the PE to
approve or decline. The existing delete-heavy fold is strongly reinforced (7 of 7
predicted takeovers) but needs no text change.

### Survey basis (2026-07-07)

M4 was scoped from a full architect read of the compaction path
(`digest.rs`, `session.rs`, `server/ask.rs:241-333`, `stream.rs` write-back,
`prompt.rs`, `event_log.rs`, `ghost.rs`). The complete failure catalog
(D1–D15) with file:line evidence lives in the design doc §2 — the phase docs
cite defects by D-number rather than restating them.

Two survey corrections folded into scope:

- The FTS5 memory index described in `docs/architecture.md` §"Knowledge
  system" is a **stub** (`src/memory/index.rs` returns empty); real search is
  the grep scan in `src/search.rs`. M4 builds `recall_context` on the grep
  idiom; FTS5 remains future work.
- No sqlite/zstd dependencies exist in `Cargo.toml`; M4 adds none.

### Calibration carry-ins (apply when re-validating phase docs at dispatch)

- **Front-load by task shape** (WORKFLOW.md): core design phases carry the
  load-bearing constraint + a worked example quoted from the codebase;
  mechanical phases run at normal density.
- **Prefer additive shapes** (WORKFLOW.md): new files, new config sections
  with serde defaults, sibling functions. The two deliberate multi-site
  migrations (phase 01 event readers; phase 03 threshold constants) carry
  grep-verified ordered site lists with build-after-each-site instructions.
- **Wired-in state needs a consumer** (WORKFLOW.md § Derive intentionally):
  phase 02 (estimation) is consumed by phase 03; phase 04 (archive) by
  phase 07 (recall). Each producing phase's doc names its consumer so the
  executor understands why the state exists.

### Candidate work held out of M4

- FTS5 transcript index + un-stubbing `src/memory/index.rs` (needs rusqlite).
- zstd segment compression for archives/events.
- Cross-session recall.
- The two M3 survey holdovers (error-result/response-builder helper ~74
  sites; executor approval-gate extraction) remain deferred.

### rexyMCP runtime feature requests — IMPLEMENTED 2026-07-14 (ahead of schedule, PE request)

Both landed on rexyMCP master before resuming M4: **FR-1** = `a9399a0`
(`git stash` guard), **FR-2** = `2a405a7` (no-progress read-only stall detector,
`[governor] read_only_stall_threshold`, default 20). Requires a `rexymcp serve`
rebuild + restart to take effect for future dispatches. Original spec below.

#### (original) rexyMCP runtime feature requests

Filed 2026-07-14 from the M4 executor-pathology calibration fold (WORKFLOW.md
§ "Executor self-sabotage on delete-heavy rewrites is a runtime concern").
These are **rexyMCP-product** changes (not DaemonEye code); M4 caused 3 phase
takeovers because of them (phases 01, 03, 05b).

1. **FR-1 — Hard-block the executor from reverting its own uncommitted work.**
   The current guard only *warns* on `git checkout <file>` and misses
   `git checkout HEAD -- <file>` and `git stash`, which the executor used to
   wipe correct work on phases 01 and 03. Fix: deny (not warn) any
   `git checkout|restore|reset|stash` that would discard the executor's own
   uncommitted changes from the current run — covering the `HEAD -- <path>`,
   bare-`<path>`, and `stash` forms. Ideal: auto-create a throwaway checkpoint
   commit before allowing it, so nothing is ever lost. **Severity: high** — this
   is the single biggest cause of M4 executor hard_fails.

2. **FR-2 — Broaden the loop governor to catch near-identical verify-loops.**
   The identical-call/oscillation governor caught the exactly-identical case on
   phase-05a (6 calls) but MISSED phase-05b, which looped 529 turns until a human
   `rexymcp stop`. Fix: normalize whitespace/argument ordering before the
   identical-call comparison, and trip on *N consecutive read-only calls
   (`grep`/`git status`/`cargo test`) with zero intervening file writes*.
   **Severity: high** — without it, verify-loops burn a full run and require
   human intervention to stop.