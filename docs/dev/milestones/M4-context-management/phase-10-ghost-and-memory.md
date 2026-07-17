# Phase 10: Ghost-and-memory — SPLIT into 10a + 10b (redirect)

**Status:** superseded (2026-07-16)

This phase was re-split into two independent, single-subsystem phases because it
bundled two unrelated features (ghost-loop compaction vs. epoch-build memory
extraction) across ~6 files — the multi-subsystem shape this executor repeatedly
stalled on. Mirrors the earlier 05a/05b split.

- **[phase-10a-ghost-coverage.md](phase-10a-ghost-coverage.md)** — synchronous,
  model-call-free ghost working-set guard (`enforce_ghost_working_set` +
  `ghost.rs` wiring). Dispatch-ready.
- **[phase-10b-memory-extraction.md](phase-10b-memory-extraction.md)** — opt-in
  compaction→memory extraction (`extract_memories` flag + hook into the async
  epoch build). Carries this doc's §2/§3 spec; **re-verify its anchors before
  dispatch** (known issues flagged in the doc).

Do not dispatch this file.
