# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).
Status `in-progress` — phases 01–10 are `done`; phase-10 was approved on review
(`approved_after_2`, 2026-06-26; bug-phase-10-1 + bug-phase-10-2 both resolved). More
phases remain before the milestone closes (one more TUI input/UX fix, then the rest of
the C5 split sweep).

**Active phase:** **none currently dispatched.** Next up is **phase-11 —
interrupt-and-colors** (two-press ESC/Ctrl+C agent interrupt + blood-red/deep-yellow
`commit_panel` recolor), the second of the two TUI input/UX fix phases inserted ahead of
the remaining C5 splits. Draft on demand via `/rexymcp:architect next`.

phase-10 — input-editor is `done`: a visible cursor + word-wrap + multi-line input +
multi-line paste + internal scroll in the ratatui input box. Took the full M2 calibration
ladder — two bounces (green-but-inert: first the wrong tty/render seams, then correct
code but the seam still untested) before a third dispatch added hermetic `read_key` seam
tests over a pipe-injected `from_raw_fd`. The live-terminal interactive E2E remains a
recommended one-time manual PE confirmation (see the phase doc's approved Review verdict).

phase-09 — split-config is `done` (approved_after_1, 2026-06-26; bug-09-1 dropped
6 doc-comment lines in the verbatim split, fixed).

**Next after phase 10:** phase-11 — interrupt-and-colors (two-press ESC/Ctrl+C
agent interrupt + blood-red/deep-yellow `commit_panel` recolor). Draft on demand
via `/rexymcp:architect next`.

The C5 split sweep resumes at **phase 12** (split-file-ops): split every remaining
source file over 1000 lines, biggest first, toward a ~600-line target. Order:
12 `daemon/executor/file_ops.rs` → 13 `ai/types.rs` → 14 `daemon/background.rs` →
15 `daemon/executor/knowledge.rs`. All four are drafted as rows in the M2 README
phase table; full phase docs are drafted on demand via `/rexymcp:architect next`.

Phase order so far (01–09 all done): 01 ✓ → 02a ✓ → 02b ✓ → 03 ✓ → 04 split-render ✓ →
05 split-input ✓ → 06 split-commands ✓ → 07 split-tools ✓ → 08 split-server ✓ →
09 split-config ✓ → **10 input-editor (next to dispatch)**.

**Deferred (until M2 closes):** the calibration fold into WORKFLOW.md (make front-loading
task-shape-conditional) — drafted in the M2 README "Interim calibration findings", on
hold per PE 2026-06-26.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
