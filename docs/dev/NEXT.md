# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).

**Active phase:** none drafted — **06 split-commands is next to draft.** Run
`/rexymcp:architect next` to author `phase-06-split-commands.md`, then `/rexymcp:dispatch 06`.

Phase 05 (split-input) is `done` (approved_first_try — Qwen/Qwen3.6-27B-FP8; faithful
byte-for-byte move verified by 374-vs-369 sorted multiset line diff — only delta is the 2
authorized section-banner comments + 3 blank lines; `cli/input.rs` 374 → `cli/input/`
submodule `tty.rs` 224 + `editor.rs` 145 + `mod.rs` 5; zero caller edits; no new tests, as
specced). Phase 04 (split-render) is `done` (approved_first_try — Qwen/Qwen3.6-27B-FP8;
1051-vs-1051 multiset line diff; `render.rs` 1365 → 234 lines). Phase 03
(retire-legacy-and-verify) is `done` (escalated — session takeover after 2 hard_fails).

Phase order: 01 ✓ → 02a ✓ → 02b ✓ → 03 ✓ → 04 split-render ✓ → 05 split-input ✓ →
**06 split-commands**.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
