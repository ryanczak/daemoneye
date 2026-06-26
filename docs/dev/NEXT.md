# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).

**Active phase:** 04 — split-render (`phase-04-split-render.md`), status `todo` — **drafted
2026-06-25, ready to dispatch.** Mechanical extraction of the markdown + syntax-highlight
half of `cli/render.rs` (1365 lines) into a new `cli/markdown/` submodule (`mod.rs` +
`syntax.rs`); pure move, no behavior change, guarded by the 4 existing markdown tests.
Normally-specced (not lean) per the M2 calibration protocol — splits 04–06 are
low-complexity and excluded from the spec-density experiment.
Phase 03 (retire-legacy-and-verify) is `done` (escalated — session takeover after 2
hard_fails; all 6 sub-deliverables completed by architect directly; deletion-completeness
grep clean, clippy zero warnings, 27 integration tests passing, `window_switch_does_not_corrupt_chat`
E2E test added as `#[ignore]`).

Phase order: 01 ✓ → 02a ✓ → 02b ✓ → 03 ✓ → **04 split-render** → 05 split-input →
06 split-commands.
Dispatch phase 04 with `/rexymcp:dispatch 04`; review with `/rexymcp:review 04`.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
