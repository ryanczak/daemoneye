# NEXT

**Active milestone:** M2 — TUI Renderer Overhaul (`docs/dev/milestones/M2-tui-renderer/`).
Status `in-progress` — phases 01–06 are `done`, but more phases remain before the
milestone closes.

**Active phase:** none drafted. **phase-07 — split-tools** is `done`
(approved_after_1, 2026-06-26; bug-phase-07-1 resolved). The next phase is
**phase-08 — split-server** (`daemon/server.rs`, 1976 lines), a README phase-table
row not yet expanded into a full phase doc. Draft it via `/rexymcp:architect next`,
then dispatch via `/rexymcp:dispatch phase-08`.

Phases 07–13 are the broader C5 sweep: split every remaining source file over 1000
lines, biggest first, toward a ~600-line target. Order: 07 `ai/tools.rs` →
08 `daemon/server.rs` → 09 `config.rs` → 10 `daemon/executor/file_ops.rs` →
11 `ai/types.rs` → 12 `daemon/background.rs` → 13 `daemon/executor/knowledge.rs`.
All seven are drafted as rows in the M2 README phase table; only phase-07 has a
full phase doc so far (the rest are drafted on demand via `/rexymcp:architect next`).

Phase order so far (01–07 all done): 01 ✓ → 02a ✓ → 02b ✓ → 03 ✓ → 04 split-render ✓ →
05 split-input ✓ → 06 split-commands ✓ → 07 split-tools ✓ → **08 split-server (next to draft)**.

**Deferred (until M2 closes):** the calibration fold into WORKFLOW.md (make front-loading
task-shape-conditional) — drafted in the M2 README "Interim calibration findings", on
hold per PE 2026-06-26.

M1 (Agent Tooling Improvements) is **complete** — all eleven phases `done`; see its
retrospective in `docs/dev/milestones/M1-agent-tooling/README.md`.
