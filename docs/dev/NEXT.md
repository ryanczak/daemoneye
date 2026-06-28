# NEXT

**Active milestone:** **M3 — Polish & Maintenance** (kicked off 2026-06-27). Goal: pay
down post-M2 debt — correctness/hermeticity bugs, user-facing rough edges, and codebase
health — with no behavior regressions. Open design scope (breaking wire/format/architecture
changes flagged per-phase for PE sign-off). See
`docs/dev/milestones/M3-polish-maintenance/README.md` for the phase plan and survey basis.

**Active phase:** **phase-06 — error-hardening** (`todo`, drafted 2026-06-28). Doc:
`docs/dev/milestones/M3-polish-maintenance/phase-06-error-hardening.md`. Dispatch it
with `/rexymcp:dispatch phase-06`.

Phase-06 scope (bug, `daemon/memory_prompt.rs` + `ai/mod.rs` + `daemon/scheduled.rs`):
three behavior-preserving hardening edits the M3 survey found — (1) rewrite the
`memory_prompt.rs:91` `get_mut(...).unwrap()` as an Entry-API assignment; (2) convert the
four `ai/mod.rs` circuit-breaker `unwrap_or_else(|e| e.into_inner())` lock sites to the
documented `.unwrap_or_log()` invariant (adds ERROR-on-poison logging); (3) make the five
`daemon/scheduled.rs` swallowed `notify_tx` sends `log::debug!` on a dropped receiver instead
of `let _ =`. The `tmux`/`ai` "risky unwrap" audit came back clean — the remaining unwraps are
invariant-proven (compile-time regexes, capture group 0, default reqwest client) and are pinned
as must-NOT-touch in the doc. No new tests (equivalence rewrites; existing circuit-breaker
tests cover the lock sites). ~40 lines.

The remaining M3 phases are recorded as `todo` rows in the M3 README phase table and are drafted on
demand via `/rexymcp:architect next` after each prior phase is approved.

---

**M3 phase-05 — consolidate-leaf-params is `done`** (approved_first_try, 2026-06-28).
Introduced per-function borrow-structs (`UpdateMemoryArgs`, `SaveSessionArgs`, `RunEditArgs`,
`UpdateMemoryRequest`, `CreateAgentArgs`) resolving 5 of the 7 `TODO(M2)` markers and deleting
their `#[allow(clippy::too_many_arguments)]` suppressions. Executor commit `822ba7f`; review
approval `e89255e`.

**M3 phase-04 — error-message-quality is `done`** (approved_first_try, 2026-06-28). Killed the
`render_error` `{:?}` debug-dump leak via an exhaustive `Response::kind()` label method + a pure
`error_line()` formatter (`unexpected reply from daemon (<Kind>)`), and normalized the
`/session list` + `/prompt` empty-state strings. Executor commit `77ee226`; review approval `1b9d22f`.

**M3 phase-03 — split-utils is `done`** (approved_first_try, 2026-06-28). Split the 1007-line
`src/daemon/utils.rs` grab-bag into a `daemon/utils/` directory of cohesive submodules with
`pub use <submod>::*;` re-exports preserving every `crate::daemon::utils::<name>` path. Executor
commit `bc4b76f`; review approval `4a69f1e`.

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
