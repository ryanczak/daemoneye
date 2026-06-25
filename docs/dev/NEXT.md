# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).

**Active phase:** 02b — tools-and-default (`phase-02b-tools-and-default.md`), status
`todo`, ready to dispatch. Phase 02a (streaming-markdown) is `done` (approved_after_1 —
one bounce, bug-phase-02a-1, green-but-inert: tokens streamed to stdout instead of
scrollback; cleared at rung 1.5 with a single call-site integration pin).

02b is the **hardest integration in M2**: interactive tool-call approval through the
ratatui renderer while crossterm owns raw mode (raw/cooked-mode coexistence), plus tool
panels, the README-tracked pre-flip code-block-state fix, and flipping the
`DAEMONEYE_RENDERER` default from `legacy` to `ratatui`. Interactive approval and the
default-flip are coupled (the ratatui path auto-denies tools today, so flipping without
interactive approval would silently break all tool use) — that's why they move together
here. See the M2 README "Phase 02 split into 02a + 02b" and "Pre-02b follow-up" notes.

02b is a LEAN rewrite-phase spec (calibration protocol: pin what + acceptance +
boundaries; executor self-discovers the ratatui/crossterm API; the verify-against-live-
docs Pre-flight is kept). It carries the anti-one-shot granularity pin that cleared
phases 01 and 02a.

Phase order: 01 render-core ✓ → 02a streaming-markdown ✓ → **02b tools-and-default** →
03 retire-legacy-and-verify (the corruption-fix E2E gate) → 04 split-render → 05
split-input → 06 split-commands. Dispatch phase 02b with `/rexymcp:dispatch 02b`; review
with `/rexymcp:review 02b`.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
