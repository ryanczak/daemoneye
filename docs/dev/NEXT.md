# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).
Status `in-progress` — phases 01–11 are `done`. The remaining work is the C5 split sweep
(phases 12–15).

**Active phase:** **none currently dispatched.** Next up is **phase-12 — split-file-ops**
(split `daemon/executor/file_ops.rs`, 1475 lines). Draft on demand via
`/rexymcp:architect next`.

phase-11 — interrupt-and-colors is `done` (**escalated — architect takeover**, 2026-06-27).
Two-press ESC/Ctrl+C interrupt of a streaming turn + blood-red/deep-yellow `commit_panel`
recolor. Took the full M2 calibration ladder on the interrupt (tokio-concurrency) seam:
rung-0 → bounce (bug-11-1: recreated the read future inside `select!`) → bounce (bug-11-2:
held the future but returned-and-dropped it on warn/tick) → hard_fail (governor:
IdenticalToolCallRepetition while patching the callback fix) → takeover. The takeover fix
moved partial-read state out of the droppable future into a caller-owned `Vec<u8>`
(`recv_line` via `read_until`), making interruption non-destructive, and added the
mutation-verified seam regression test both bugs demanded. The color half landed first try
every dispatch. Live-terminal interactive E2E remains a recommended one-time manual PE
confirmation (see the phase doc's final Review verdict). **Third strong M2 data point that
front-loading should become task-shape-conditional** — folded into the still-deferred M2
retrospective.

phase-10 — input-editor is `done`: a visible cursor + word-wrap + multi-line input +
multi-line paste + internal scroll in the ratatui input box. Took the full M2 calibration
ladder — two bounces (green-but-inert: first the wrong tty/render seams, then correct
code but the seam still untested) before a third dispatch added hermetic `read_key` seam
tests over a pipe-injected `from_raw_fd`. The live-terminal interactive E2E remains a
recommended one-time manual PE confirmation (see the phase doc's approved Review verdict).

phase-09 — split-config is `done` (approved_after_1, 2026-06-26; bug-09-1 dropped
6 doc-comment lines in the verbatim split, fixed).

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
