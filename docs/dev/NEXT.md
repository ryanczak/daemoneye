# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).
Status `in-progress` — phases 01–06 are `done`, but more phases remain before the
milestone closes.

**Active phase:** **phase-07 — split-tools**
(`docs/dev/milestones/M2-tui-renderer/phase-07-split-tools.md`, status `todo`).
Splits `ai/tools.rs` (2232 lines) into a `tools/` submodule (schema / defs / args /
dispatch). Dispatch via `/rexymcp:dispatch phase-07`.

Phases 07–13 are the broader C5 sweep: split every remaining source file over 1000
lines, biggest first, toward a ~600-line target. Order: 07 `ai/tools.rs` →
08 `daemon/server.rs` → 09 `config.rs` → 10 `daemon/executor/file_ops.rs` →
11 `ai/types.rs` → 12 `daemon/background.rs` → 13 `daemon/executor/knowledge.rs`.
All seven are drafted as rows in the M2 README phase table; only phase-07 has a
full phase doc so far (the rest are drafted on demand via `/rexymcp:architect next`).

Phase order so far (01–06 all done): 01 ✓ → 02a ✓ → 02b ✓ → 03 ✓ → 04 split-render ✓ →
05 split-input ✓ → 06 split-commands ✓ → **07 split-tools (active)**.

**Deferred (until M2 closes):** the calibration fold into WORKFLOW.md (make front-loading
task-shape-conditional) — drafted in the M2 README "Interim calibration findings", on
hold per PE 2026-06-26.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
