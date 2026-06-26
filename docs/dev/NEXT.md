# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).

**Active phase:** none drafted — **05 split-input is next to draft.** Run
`/rexymcp:architect next` to author `phase-05-split-input.md`, then `/rexymcp:dispatch 05`.

Phase 04 (split-render) is `done` (approved_first_try — Qwen/Qwen3.6-27B-FP8; faithful
mechanical move verified by 1051-vs-1051 multiset line diff; `render.rs` 1365 → 234 lines;
new `cli/markdown/` submodule `mod.rs` 746 + `syntax.rs` 386; 4 relocated markdown tests
green; one cosmetic deviation — a redundant syntax-section banner comment dropped). Phase
03 (retire-legacy-and-verify) is `done` (escalated — session takeover after 2 hard_fails).

Phase order: 01 ✓ → 02a ✓ → 02b ✓ → 03 ✓ → 04 split-render ✓ → **05 split-input** →
06 split-commands.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
