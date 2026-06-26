# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).
Status `in-progress` — phases 01–09 are `done`; phase-10 **bounced on review**
(bug-phase-10-1) and is back to `in-progress`. More phases remain before the
milestone closes (two TUI input/UX fixes, then the rest of the C5 split sweep).

**Active phase:** **phase-10 — input-editor**
(`docs/dev/milestones/M2-tui-renderer/phase-10-input-editor.md`), `in-progress`
(bounced 2026-06-26 — see bug-phase-10-1). First of two **TUI input/UX fix phases**
inserted ahead of the remaining C5 splits (PE direction 2026-06-26; see the M2 README
"UI-fix insertion" note). Delivers a **visible cursor + word-wrap + multi-line input +
multi-line paste** in the ratatui input box, which is today a single-line buffer with
no cursor, no wrapping, and submits a pasted block at its first newline. Specced
**LEAN** on purpose (design-discovery; extends the M2 calibration dataset).
**Bounce:** three core acceptance criteria (deliberate newline, multi-line paste,
internal scroll) are green-but-inert on a real terminal — the executor reached for the
wrong tty/render seams and self-declared completion. Re-dispatch via
`/rexymcp:dispatch phase-10` once bug-phase-10-1 is fixed.

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
