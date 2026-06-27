# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).
Status `in-progress` — phases 01–12 are `done`. The remaining work is the C5 split sweep
(phases 13–15).

**Active phase:** **phase-13 — split-types** is **drafted and ready to dispatch**
(`docs/dev/milestones/M2-tui-renderer/phase-13-split-types.md`, status `todo`). A verbatim
move split of `ai/types.rs` (1413 lines) into `types/` : `mod` (re-exports only), `wire`
(`ToolCall`/`ToolResult`/`Message`/`TokenBreakdown` + custom deserializer), `pending`
(`PendingCall` + its impl), `events` (`AiEvent`). Specced **NORMAL** (mechanical split — the
04–06/08/09/12 pattern that cleared first try). Even simpler than phase 12: the file uses **no**
`super::`/`crate::` paths so there is no `super:: → super::super::` re-pathing, and every item
is already `pub` so there are **no visibility bumps** — only two internal sibling imports
(`pending`→`wire::ToolCall`, `events`→`wire::TokenBreakdown`) and the `mod.rs` re-exports.
`pending.rs` is authorized to land ~900 lines (single cohesive enum + impl, the `defs.rs`-style
exception). Sorted-multiset line diff is the fidelity gate. Dispatch via
`/rexymcp:dispatch phase-13`.

phase-12 — split-file-ops is `done` (**approved_first_try**, 2026-06-27). Verbatim
move-and-re-path split of `daemon/executor/file_ops.rs` (1475) into `file_ops/{mod,read,write,
ops}.rs`. One justified scope note: `EditArgs`/`run_edit_file`/`run_read_file` needed `pub`
(not the spec's `pub(super)`) for the `pub(super) use` re-export to compile (E0364) — folded
forward into phase-13's spec, which pins re-export visibility explicitly. **Fourth clean
mechanical C5 split** — confirms NORMAL spec density clears first try for verbatim splits.

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

The C5 split sweep continues at **phase 13** (split-types): split every remaining
source file over 1000 lines, biggest first, toward a ~600-line target. Order:
12 `daemon/executor/file_ops.rs` ✓ → 13 `ai/types.rs` (drafted) → 14 `daemon/background.rs` →
15 `daemon/executor/knowledge.rs`. Phases 14–15 are drafted as rows in the M2 README
phase table; full phase docs are drafted on demand via `/rexymcp:architect next`.

Phase order so far (01–12 all done): 01 ✓ → 02a ✓ → 02b ✓ → 03 ✓ → 04 split-render ✓ →
05 split-input ✓ → 06 split-commands ✓ → 07 split-tools ✓ → 08 split-server ✓ →
09 split-config ✓ → 10 input-editor ✓ → 11 interrupt-and-colors ✓ → 12 split-file-ops ✓ →
**13 split-types (next to dispatch)**.

**Deferred (until M2 closes):** the calibration fold into WORKFLOW.md (make front-loading
task-shape-conditional) — drafted in the M2 README "Interim calibration findings", on
hold per PE 2026-06-26.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
