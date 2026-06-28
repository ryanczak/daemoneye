# M3 — Polish & Maintenance

**Goal:** Pay down the post-M2 debt — fix correctness/hermeticity bugs, smooth the
user-facing rough edges, and improve codebase health (module cohesion, signature
clarity, test coverage) — without regressing any shipped behavior.

**Status:** planning

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
| 07 | split-webhook — split `webhook.rs` (1210) into payload-parsing vs HTTP-handler/dedup submodules | maint | todo |
| 08 | help-and-truncation — ellipsis markers on silent truncation (status bar / panel / committed text); `/help` completeness (aliases, document redirect + tool-output cap) | ux | todo |
| 09 | consolidate-loop-ctx — `ConversationLoopCtx` + `AskRequest`/`AskContext` for the two high-arity orchestration fns (`run_conversation_loop`, `handle_ask`) | maint | todo |
| 10 | knowledge-tests — add unit tests to `executor/knowledge/{agents,artifacts,memory,pane}.rs` (zero coverage today) | maint | todo |

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

### Retrospective

_(Filled in at milestone close.)_
