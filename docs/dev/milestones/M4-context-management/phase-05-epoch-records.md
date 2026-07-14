# Phase 05: Epoch records — RE-SPLIT

**Status:** superseded (re-split 2026-07-14)

This phase was re-split into two executor-sized phases because at ~500 lines it
sat at the one-session limit and it deletes/replaces the phase-03 compaction path
(the exact digest-heavy shape the local executor git-thrashed on twice):

- **[phase-05a-epoch-persistence.md](phase-05a-epoch-persistence.md)** — additive:
  `context/epochs.rs` types + append-only persistence + span-windowed
  `tally_span`/`scan_artifacts_span`. Deletes/rewires nothing.
- **[phase-05b-epoch-head.md](phase-05b-epoch-head.md)** — the rewire:
  `compact_with_epochs` regenerated head, `render_context_block`, keep-newest
  narrative, and deletion of the old single-digest path.

The original design and pins live in those two docs. Do not execute this file.
