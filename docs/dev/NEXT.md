# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).

**Active phase:** 02a — streaming-markdown (`phase-02a-streaming.md`), status `todo`,
ready to dispatch. Phase 01 (render-core) is `done` (approved_after_3 — three live-E2E
bounces, each a green-but-broken integration miss `TestBackend` is blind to: banned
termios shortcut, raw mode never entered, AI answer painted over the viewport).

Phase 02 was **split into 02a + 02b** (2026-06-24, see the M2 README Notes): the
default-flip is coupled to interactive tool approval (the ratatui path auto-denies tools
today), and approval's raw/cooked-mode coexistence is M2's hardest integration — so they
move together in 02b, separate from streaming. **02a** lands streamed markdown + a
live-region spinner + resize redraw behind the flag (default stays legacy, tools stay
auto-denied). **02b** adds interactive tool approval + tool panels and flips the default.

Both 02a/02b are LEAN rewrite-phase specs (calibration protocol: pin what + acceptance +
boundaries; executor self-discovers the ratatui API; the verify-against-live-docs
Pre-flight is kept). Each carries the anti-one-shot granularity pin that cleared phase 01.

Phase order: 01 render-core ✓ → **02a streaming-markdown** → 02b tools-and-default → 03
retire-legacy-and-verify (the corruption-fix E2E gate) → 04 split-render → 05 split-input
→ 06 split-commands. Dispatch phase 02a with `/rexymcp:dispatch 02a`; review with
`/rexymcp:review 02a`.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
