# Phase 05c: reconcile scope — an empty corpus must not wipe the others

**Milestone:** M11 — Unified Knowledge Index
**Status:** todo — **BLOCKED: needs a PE design decision before dispatch**
**Depends on:** phase-05b (done — surfaced the defect)
**Estimated diff:** unknown until the design decision is made
**Tags:** language=rust, kind=bugfix, size=unknown

## Goal

Stop a search over an empty corpus from destroying every other corpus in the
index. Full defect analysis, reproduction, and production impact are in
[bug-05c-1](bugs/bug-05c-1.md) — read that first; this doc does not restate it.

## Why this is blocked

**The fix changes `reconcile_index()`'s contract, and there are two viable shapes
with different trade-offs. That choice is a design decision, not an
implementation detail, so it is the PE's to make — the autonomous loop stopped
here rather than pick one.**

### Option 1 — scope the reconcile to one corpus

`open_and_reconcile_if_empty("memories")` rebuilds only `memories`.

- *For:* preserves the self-healing property `fts5_search` has had since M7, and
  fixes the bug at its root.
- *Against:* `reconcile_index()` currently clears and rebuilds all seven tables
  inside one transaction. Splitting it per corpus is the larger change, and the
  contentless corpora (`turns`, `events`) share their `_map` tables' lifecycle,
  so "rebuild only turns" has to keep `turns_map` consistent.

### Option 2 — drop reconcile-on-empty from the three newer searches

Keep it only on `fts5_search`, and rely on the incremental hooks (03a/03b) plus
`daemoneye reindex` to keep the newer corpora current.

- *For:* small, surgical, no contract change.
- *Against:* loses self-healing for `artifacts` / `turns` / `events` / `epochs`.
  A fresh install would return no artifact hits until the first write hook fires
  or the operator runs `daemoneye reindex`.

### A third question either way

**An empty corpus is not evidence of a stale index.** A user who genuinely has
zero memories triggers a full rebuild on *every* search. Whichever option is
chosen should also decide whether "empty" is the right trigger at all, or whether
reconcile-on-empty should fire at most once per process.

## What is already known

- Reproduced mechanically; transcript in [bug-05c-1](bugs/bug-05c-1.md).
- Phase 05b's guard test currently seeds **every** corpus to work around this.
  That workaround should be removable once this phase lands, and its removal is
  the natural acceptance test — see the bug doc's Verification section.
- Phase 06 (prompt scoring) reads the index on every turn and would be exposed to
  the same wipe, so **05c should land before 06**.

## Next step

PE picks Option 1, Option 2, or a third approach. Once decided, this doc gets a
real Spec / Acceptance criteria / Test plan and is dispatched normally.

## Update Log

<!-- entries appended below this line -->
