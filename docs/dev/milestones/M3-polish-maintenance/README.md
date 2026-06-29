# M3 — Polish & Maintenance

**Goal:** Pay down the post-M2 debt — fix correctness/hermeticity bugs, smooth the
user-facing rough edges, and improve codebase health (module cohesion, signature
clarity, test coverage) — without regressing any shipped behavior.

**Status:** done

**Depends on:** M2 (TUI Renderer Overhaul) — complete.

**Exit criteria:**

- [ ] No known flaky tests: `cargo test` is green across repeated parallel runs;
      every test that mutates `HOME`/env holds `TEST_HOME_LOCK`.
- [ ] The tool-call approval prompt presents one consistent format and option
      order across the tool / runbook / `edit_file` flows.
- [ ] No user-facing error path prints a `{:?}` debug dump of an internal enum.
- [ ] No source file over ~1000 lines is a low-cohesion grab-bag (the remaining
      large files are single cohesive units by deliberate decision, recorded in
      Notes).
- [ ] The 7 `TODO(M2): consolidate params into a struct` markers are resolved.
- [ ] The `executor/knowledge/` artifact + agent + memory + pane handlers have
      unit-test coverage (they have none today).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` stays clean; the
      `too_many_arguments` suppressions removed by M3 are gone, not re-added.

## Architecture references

- `docs/architecture.md#1-system-layers` — the 4-layer decomposition the
  maintenance phases must preserve (no new layer violations).
- `docs/architecture.md#21-interactive-requestresponse` — the approval-prompt
  flow the UX phases touch.
- `docs/architecture.md#24-remote-host-execution-model` — invariant the
  `edit_file` / executor refactors must not break.

## Scope notes

- **Open design scope (per PE, 2026-06-27).** Targeted architecture, IPC-protocol,
  and on-disk-format changes are permitted where they fix a real pain point. Any
  *breaking* wire/format/architecture change is flagged in its phase doc for
  explicit PE sign-off before dispatch; `docs/architecture.md` is updated in the
  same phase that changes the design.
- **Even-split intent, readiness-ordered execution.** The three themes — bugs,
  UX, maintenance/design — are weighted equally in intent. The phase *sequence*
  interleaves them and front-loads the low-risk mechanical splits (M2 showed
  verbatim C5-style splits clear first-try). Maintenance carries more rows only
  because it has the deepest concrete backlog; bugs and UX lead the order.
- **Phases are expanded on demand.** The table below is the plan of record, not a
  commitment to exact boundaries. Each phase doc is drafted via
  `/rexymcp:architect next` just before dispatch, when the prior phase's work is
  on disk to inform it. Rows may be re-split if a phase exceeds one executor
  session (~500 lines of diff).

## Phases

| #  | Phase | Theme | Status |
|----|-------|-------|--------|
| 01 | fix-test-hermeticity ([phase-01-fix-test-hermeticity.md](phase-01-fix-test-hermeticity.md)) — fix the racy `webhook_alert_to_event_log` + restore `HOME` in the 5 leak tests | bug | done |
| 02 | approval-prompt-consistency ([phase-02-approval-prompt-consistency.md](phase-02-approval-prompt-consistency.md)) — one prompt format + option order across the tool / runbook / `edit_file` approval flows | ux | done |
| 03 | split-utils — split `daemon/utils.rs` (1007, grab-bag) into cohesive submodules (shell-escape / sudo / event-log / output / response) | maint | done |
| 04 | error-message-quality ([phase-04-error-message-quality.md](phase-04-error-message-quality.md)) — kill the `render_error` `{:?}` debug-dump leak via `Response::kind()`; standardize the three slash-command empty-state messages | ux | done |
| 05 | consolidate-leaf-params ([phase-05-consolidate-leaf-params.md](phase-05-consolidate-leaf-params.md)) — resolve the low-blast `TODO(M2)` markers via param structs (`memory`, `session_store`, `knowledge/{agents,memory}`, `file_ops/ops`) | maint | done |
| 06 | error-hardening ([phase-06-error-hardening.md](phase-06-error-hardening.md)) — `memory_prompt.rs` unwrap → Entry API; `ai/mod.rs` circuit-breaker locks → `.unwrap_or_log()`; `daemon/scheduled.rs` swallowed notify sends → debug-logged. (`tmux`/`ai` unwrap audit came back clean — invariant-proven only.) | bug | done |
| 07 | split-webhook ([phase-07-split-webhook.md](phase-07-split-webhook.md)) — split `webhook.rs` (1210) into `parse` / `process` / `server` submodules | maint | done |
| 08 | help-and-truncation — ellipsis markers on silent truncation (status bar / panel / committed text); `/help` completeness (aliases, document redirect + tool-output cap) | ux | done |
| 09 | consolidate-loop-ctx ([phase-09-consolidate-loop-ctx.md](phase-09-consolidate-loop-ctx.md)) — `ConversationLoopCtx` + `AskRequest`/`AskContext` for the two high-arity orchestration fns (`run_conversation_loop`, `handle_ask`); resolves the final 2 `TODO(M2)` markers | maint | done |
| 10 | knowledge-tests — add unit tests to `executor/knowledge/{agents,artifacts,memory,pane}.rs` (zero coverage today) | maint | done |

## Notes

### Survey basis (2026-06-27)

M3 was scoped from a four-angle survey of the codebase at the M2/M3 boundary
(hard-fact pass + three parallel Explore surveys: UX, correctness, maintenance):

- **Bugs are thin** — the codebase is robust. The concrete correctness findings
  are: the documented parallel-`HOME` flaky test (no `TEST_HOME_LOCK`); swallowed
  channel-send `Result`s in `daemon/scheduled.rs`; a `memory_prompt.rs`
  unwrap-after-`or_insert` better written via the Entry API; and a
  TOCTOU on `pane_exists`→`start_pipe_pane` that is **already mitigated** (no phase
  needed). This is why bugs carry only 2 phases.
- **UX papercuts** cluster in the approval-prompt inconsistency (three flows, three
  option orders), the `render_error` debug-dump leak, silent truncation without
  ellipsis, and incomplete `/help`.
- **Maintenance** has the deepest backlog: `daemon/utils.rs` and `webhook.rs` are
  genuine grab-bags (most other >1000-line files — `cli/commands/stream.rs`,
  `executor/foreground.rs`, `render_ratatui.rs`, `daemon/stream.rs`, `digest.rs`,
  `ghost.rs` — are single cohesive units and are **deliberately left whole**); the
  7 `TODO(M2)` high-arity signatures; and zero test coverage in the
  `executor/knowledge/` handlers.

### Candidate phases held out of the committed table

Surfaced by the survey, deferred unless a committed phase reveals the need:

- **Error-result / response-builder helper** — ~74 sites of
  `Ok(ToolCallOutcome::Result(format!("Error: {e}")))` and repeated
  `send_response_split(tx, Response::ToolResult(…))`. A real design seam, but a
  wide-blast additive-then-migrate change; revisit after the `split-utils` and
  param-struct phases settle the surrounding code.
- **Extract the executor approval gate** (`executor/mod.rs` →
  `executor/approval.rs`) — self-contained ~150-line extraction; low priority.

### Calibration carry-ins from M2 (apply when drafting M3 phases)

- **Front-loading is task-shape-conditional** (WORKFLOW.md): the mechanical splits
  (03, 07) get NORMAL spec density with full move-and-re-path pinning — no
  design-discovery front-loading; the param-struct and ctx phases (05, 09) are
  mechanical-to-moderate. Only a phase that hides a load-bearing design decision
  gets a front-loaded constraint paragraph.
- **C5-split idiom** (M2 phases 04/05/06/12/13/14, six consecutive first-try
  clean): partition `use` statements per-target-submodule including mid-file /
  function-body imports; bump consumer-facing leaf fns `pub(super)` → `pub` where a
  `pub(super) use` re-export needs it (the E0364 fix); pin the item→submodule table
  and visibility list explicitly. Reuse this idiom verbatim for 03 and 07.
- **Prefer additive shapes** (WORKFLOW.md): the param-struct phases (05, 09) and any
  `edit_file`/IPC touch should add siblings / `#[serde(default)]` fields rather than
  mutate wide-blast types; if a multi-site mutation is unavoidable, the phase doc
  carries a grep-verified ordered site list with build-after-each-site instructions.

### Retrospective (2026-06-28)

**Outcome: 10/10 phases `done`, all `approved_first_try`. Zero bounces, zero bug
reports filed, zero review-stage escalations across the milestone.** Executor:
`Qwen/Qwen3.6-27B-PrismaAURA` (the FP8 → PrismaAURA model swap recorded in
`rexymcp.toml` carried through M3 cleanly — no regression in first-try rate vs. M2).

**All seven exit criteria met:**

1. No known flaky tests; every `HOME`/env-mutating test holds `TEST_HOME_LOCK` —
   phase-01 (fix-test-hermeticity), reconfirmed by phase-10's `with_home` idiom.
2. One consistent tool-call approval format + option order — phase-02
   (`build_approval_prompt()`, canonical `[Y]es [A]pprove for <label> [N]o`).
3. No user-facing error path prints a `{:?}` debug dump — phase-04
   (`Response::kind()` + `error_line()`).
4. No >~1000-line low-cohesion grab-bag remains — phase-03 split `daemon/utils.rs`
   (1007), phase-07 split `webhook.rs` (1210). The other large files are
   deliberately-whole cohesive units (recorded in Survey basis above).
5. All 7 `TODO(M2): consolidate params into a struct` markers resolved — phase-05
   (5 leaf params) + phase-09 (the 2 high-arity orchestration signatures).
6. `executor/knowledge/` artifact + agent + memory + pane handlers now have
   unit-test coverage — phase-10 (21 hermetic tests, four prior-zero-coverage modules).
7. `clippy --all-targets --all-features -- -D warnings` stays clean; every
   `too_many_arguments` suppression removed by M3 is gone, none re-added —
   phases 05/09.

**Theme balance (as planned, readiness-ordered):** bugs led (01, 06), UX next
(02, 04, 08), maintenance/design carried the deepest backlog (03, 05, 07, 09, 10).

**Calibration — no new folds.** M3 was an all-mechanical-to-moderate milestone
(splits, param structs, error-hardening, a test-only phase). Every phase *confirmed*
folds already in WORKFLOW.md rather than revealing a new pattern:

- The **C5-split idiom** (partition `use` per-submodule, glob re-exports, the
  E0364 `pub(super)`→`pub` bump) cleared phases 03 and 07 first-try, exactly as
  the M2 fold predicted for verbatim splits.
- **Front-load by task shape, not by default**: M3 carried no design-discovery
  phases, so specs ran at normal-to-light density; the one front-loaded phase (10)
  pre-injected a known *project* idiom (the `block_on`-inside-`with_home` async-test
  pattern that dodges `await_holding_lock`, the phase-01 trap), not a new
  workflow-general lesson.
- **Prefer additive shapes**: the param-struct phases (05, 09) used borrow-structs
  that kept the build green at every step — no multi-site mutation cascade.

Per WORKFLOW.md §Calibration, one milestone confirming existing folds is not
grounds to change the docs. **No edits to STANDARDS.md or WORKFLOW.md this
milestone.** The M2 calibration experiment (lean-spec / task-shape discriminator)
is now effectively settled: M3's 10-for-10 first-try run on maintenance-shaped
work is consistent with "mechanical phases clear first-try under normal specs."

**Carry-in for M4:** none blocking. The two survey candidates held out of M3 (the
error-result/response-builder helper ~74 sites; the executor approval-gate
extraction) remain deferred and available if an M4 phase reveals the need.
