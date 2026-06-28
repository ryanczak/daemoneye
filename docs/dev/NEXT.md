# NEXT

**Active milestone:** **M3 — Polish & Maintenance** (kicked off 2026-06-27). Goal: pay
down post-M2 debt — correctness/hermeticity bugs, user-facing rough edges, and codebase
health — with no behavior regressions. Open design scope (breaking wire/format/architecture
changes flagged per-phase for PE sign-off). See
`docs/dev/milestones/M3-polish-maintenance/README.md` for the phase plan and survey basis.

**Active phase:** **phase-03 — split-utils** (`todo`, drafted 2026-06-27). Doc:
`docs/dev/milestones/M3-polish-maintenance/phase-03-split-utils.md`. Dispatch it
with `/rexymcp:dispatch phase-03`.

Phase-03 scope (maintenance, `src/daemon/utils.rs`): split the 1007-line grab-bag into a
`daemon/utils/` directory of six cohesive submodules (`host` / `shell` / `sudo` / `event_log` /
`output` / `response`) using the M2 C5-split idiom. Pure mechanical move — every item is already
`pub` and there are no boundary-crossing internal calls, so `pub use <submod>::*;` in `mod.rs`
preserves every `crate::daemon::utils::<name>` path with zero consumer edits. Tests relocate
verbatim to per-submodule `#[cfg(test)]` blocks. No logic, signature, or behavior change.

The remaining M3 phases are recorded as `todo` rows in the M3 README phase table and are drafted on
demand via `/rexymcp:architect next` after each prior phase is approved.

---

**M3 phase-02 — approval-prompt-consistency is `done`** (approved_first_try, 2026-06-27).
Unified the three interactive approval prompts through a shared `build_approval_prompt()`
builder, canonicalizing on `[Y]es [A]pprove for <label> [N]o`. Executor commit `d4097a6`;
review approval `5726f15`.

**M3 phase-01 — fix-test-hermeticity is `done`** (approved_first_try, 2026-06-27). Converted the
racy `webhook_alert_to_event_log` to a sync `#[test]` driving its one async call via `rt.block_on`
(holds `TEST_HOME_LOCK` for the whole body, restores `HOME`), and added `HOME` capture/restore to
the five leak tests. Executor commit `c52608f`; review approval `ce7c650`. 15× concurrency soak clean.

**M2 — TUI Renderer Overhaul is complete** (2026-06-27; all 16 phases `done`). Retrospective in
`docs/dev/milestones/M2-tui-renderer/README.md`. The M2 calibration fold (front-loading made
task-shape-conditional + milestone-gate clarification) landed in WORKFLOW.md (commit `70e9712`).

**M1 — Agent Tooling Improvements is complete** — all eleven phases `done`; retrospective in
`docs/dev/milestones/M1-agent-tooling/README.md`.
