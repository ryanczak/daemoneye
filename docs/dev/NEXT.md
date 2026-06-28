# NEXT

**Active milestone:** **M3 — Polish & Maintenance** (kicked off 2026-06-27). Goal: pay
down post-M2 debt — correctness/hermeticity bugs, user-facing rough edges, and codebase
health — with no behavior regressions. Open design scope (breaking wire/format/architecture
changes flagged per-phase for PE sign-off). See
`docs/dev/milestones/M3-polish-maintenance/README.md` for the phase plan and survey basis.

**Active phase:** **phase-02 — approval-prompt-consistency** (`todo`, drafted 2026-06-27). Doc:
`docs/dev/milestones/M3-polish-maintenance/phase-02-approval-prompt-consistency.md`. Dispatch it
with `/rexymcp:dispatch phase-02`.

Phase-02 scope (UX, `src/cli/commands/stream.rs`): the three interactive approval prompts
(terminal-command, runbook write, `edit_file`) render with inconsistent option order — tool-call
uses `[Y]es [N]o [A]pprove`, the other two use `[Y]es [A]pprove [N]o`. The fix adds a single shared
`build_approval_prompt()` builder and routes all three call sites through it, canonicalizing on
`[Y]es [A]pprove for <label> [N]o` (+ optional "or type a message" where redirect is supported).
Only the tool-call prompt's visible order changes; the runbook and `edit_file` prompts render
byte-identical output. `parse_approval_response` (the input side, already consistent) is untouched.

The remaining M3 phases are recorded as `todo` rows in the M3 README phase table and are drafted on
demand via `/rexymcp:architect next` after each prior phase is approved.

---

**M3 phase-01 — fix-test-hermeticity is `done`** (approved_first_try, 2026-06-27). Converted the
racy `webhook_alert_to_event_log` to a sync `#[test]` driving its one async call via `rt.block_on`
(holds `TEST_HOME_LOCK` for the whole body, restores `HOME`), and added `HOME` capture/restore to
the five leak tests. Executor commit `c52608f`; review approval `ce7c650`. 15× concurrency soak clean.

**M2 — TUI Renderer Overhaul is complete** (2026-06-27; all 16 phases `done`). Retrospective in
`docs/dev/milestones/M2-tui-renderer/README.md`. The M2 calibration fold (front-loading made
task-shape-conditional + milestone-gate clarification) landed in WORKFLOW.md (commit `70e9712`).

**M1 — Agent Tooling Improvements is complete** — all eleven phases `done`; retrospective in
`docs/dev/milestones/M1-agent-tooling/README.md`.
