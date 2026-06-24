# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).

**Active phase:** 01 — render-core (`phase-01-render-core.md`), status `todo`, ready to
dispatch. Both kickoff process questions are now resolved (see the M2 README Notes):
the local-LLM executor runs **all** phases including the rewrite, on **lean** specs, as
a deliberate executor-ceiling probe; failures trigger graded re-dispatch (lean → +API
sketch → +example → +test skeleton → takeover). Build-green is held fixed via the
transitional `DAEMONEYE_RENDERER` switch.

M2 was kicked off 2026-06-23 to fix the long-standing chat-history corruption on
tmux window switches (root cause: DECSTBM scroll-region + absolute-positioned chrome)
by moving to `ratatui`'s committed-scrollback + inline-viewport model, and to split
the three oversized `cli/` files (C5). Locked kickoff decisions (engine = ratatui
inline; correctness-first fidelity; broad scope) are in the M2 README Notes.

Phase order: 01 render-core → 02 streaming-and-default → 03 retire-legacy-and-verify
(the corruption-fix E2E gate) → 04 split-render → 05 split-input → 06 split-commands.
Dispatch phase 01 with `/rexymcp:dispatch 01` (or the M2-scoped phase path); review
with `/rexymcp:review 01`.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
