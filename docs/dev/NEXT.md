# NEXT

**Active phase: M5 phase-01 — spinner-gutter** (`in-progress`, drafted
2026-07-24, layout revised 2026-07-25 per PE, **bounced 2026-07-25**).
Doc: `docs/dev/milestones/M5-ux-stability/phase-01-spinner-gutter.md`.

Re-dispatch with `/rexymcp:dispatch phase-01-spinner-gutter`.

**Open bugs:** `bugs/bug-01-1.md` (major — the prescribed end-to-end
verification was never run; gate output and a grep of test source were reported
as E2E evidence) and `bugs/bug-01-2.md` (minor — `render_prompt_region` draws
the prompt into the zero-height spinner rect at region height exactly 4, so a
password prompt renders with no prompt text). Gates were green at bounce (905
unit + 27 integration); the production code is otherwise accepted.

**Do not revert the `render_prompt_region` refactor** — the executor flagged it
as a spec deviation, but spec task 4 contradicted the test plan and the
deviation is the only layout that satisfies
`input_box_row_is_stable_across_draw_modes`. Recorded as `spec_bug` (architect's
fault). See the Notes-for-executor block in the phase doc.

Moves the streaming spinner out of the chat input box onto a reserved one-row
line **above** the box's top border, carrying the frame, verb, and dot
animation together. Single file (`src/cli/render_ratatui.rs`), ~150 lines, no
new dependencies, no call-site edits. The load-bearing acceptance criterion is
that the input box sits on the **same row** in all three live-region draw modes
(normal / spinner / prompt) — pinned by
`input_box_row_is_stable_across_draw_modes`.

The filename still says `spinner-gutter`; the gutter is now vertical (a
reserved row) rather than a left-hand column. Left as-is so the dispatch
identifier does not change mid-flight.

**M5 — UX & Stability is scoped** (2026-07-24, PE sign-off). Milestone README:
`docs/dev/milestones/M5-ux-stability/README.md`. Design + hang evidence log:
`docs/design/daemon-stalls.md`.

Five phases named:

01 spinner-gutter (**drafted**) → 02 echo-user-input → 03 stall-instrumentation
→ 04 unlock-blocking-paths → 05 tmux-call-hardening

Only 01 and 02 are fully specified in the design doc. **Phases 02–05 are not
yet drafted** — draft each with `/rexymcp:architect next` when its predecessor
is `done`. Phase 03's instrumentation findings may reshape 04 and 05, and the
PE chose "these three items, then reassess" at kick-off.

**Hang status (item 3).** Two mechanisms are **confirmed by code reading** and
both can wedge the whole daemon: (A) the global `SessionStore` mutex is held
across per-session disk writes *and* a blocking, timeout-free `tmux
display-message` subprocess per chat pane (`webhook/process.rs:148,161` —
`inject_ghost_event` takes the lock twice); (B) 49 blocking
`std::process::Command` tmux calls run on tokio worker threads with only 4
sites anywhere using `tokio::process`/`spawn_blocking`. **Which one fired in
the two observed 2026-07-24 restarts is NOT established** — no live wedge was
captured, and the running daemon inspected at scoping time was idle-healthy.
That is why instrumentation (03) precedes the fixes (04, 05).

- **Calibration:** the M4 candidate fold (large additive blocks → executor
  self-sabotage, from phase-10b) remains **held for recurrence** per PE. If an
  M5 phase reproduces it, that is occurrence three and the fold lands in
  `WORKFLOW.md`.

---

**M4 — Context Management Overhaul is complete** (2026-07-16, all ten phases
`done`, retrospective in
`docs/dev/milestones/M4-context-management/README.md` § Retrospective). Gates
green at close: 901 lib-unit + 27 integration passing, clippy clean.

**M4 phase-10b — memory-extraction is `done`** (2026-07-16, escalated → architect
takeover; the LAST M4 phase). Opt-in (off-by-default) memory extraction from the
interactive **async** epoch build (`extract_memories_from_epoch` in
`context/epochs.rs`, wired into `run_compaction` after `append_epoch`; category
`knowledge`, `source: "compaction"` stamped as a raw frontmatter line — no schema
change). Executor `hard_fail`ed on `LowNoveltyStall` (rexyMCP#3 governor, in the
wild) after corrupting adjacent existing code while adding the +309-line
`epochs.rs` block — the documented large-addition self-sabotage pathology. Its
production code (extraction fn, `apply_extraction`, config flag, call site) was
correct as written; takeover restored a deleted test-fn signature, removed a
stray `#[test]`/`}` pair + a duplicate `append_epoch` line, and fixed 3 test bugs
(`env::var::var` typo, `Config::load_default`, private-fn round-trip). 913 unit +
27 integration green. Every M4 epoch/compaction-path phase (03, 05a, 05b, 06, 07,
08, 10b) except 04 and 10a needed takeover.

**M4 phase-10a — ghost-coverage is `done`** (2026-07-16, approved_first_try,
commit `06389f6`). Synchronous, model-call-free ghost working-set guard
(`enforce_ghost_working_set` in new `context/ghost_ws.rs`, wired into the
`ghost.rs` turn loop). **First M4 compaction/epoch-path phase to reach `done`
WITHOUT architect takeover** — executor completed clean in 109 turns, no
git-thrash, no verify-loop. Structured-only epochs (`narrative == None`), skips
`maybe_rollup` (the one deliberate divergence from the interactive ladder, since
rollup can make a model call). 909 unit + 27 integration tests green.

**M4 phase-09 — session-meta-persistence is `done`** (2026-07-16,
approved_after_1, commit `f7e4df2`). `<id>.meta.json` continuity + boundary-safe
reload. First M4 compaction/session-path phase to reach done WITHOUT takeover
(resume+spec-fix, then one review bounce on vacuous boundary tests, bug-09-1) —
after the rexyMCP#2 governor fix. Filed rexyMCP#3 (novelty-aware stall
detection) as the follow-up.

**M4 phase-08 — async-compaction is `done`** (2026-07-15, escalated → architect
takeover after **two** `hard_fail`s, both `NoProgressStall` on the `ask.rs`
step-2 rewire — the documented Qwen git-thrash/orient-paralysis pathology).
Run 1 self-reverted `ask.rs` and thrashed; run 2 (dispatched on the partial
tree, per PE choice) burned 40 turns reading with zero edits. The executor's
scaffold was near-complete (`background.rs` +408, the `SessionEntry` fields,
the ctx thread, the narrative-default flip all correct — tree was one struct
field from building). Architect finished the last mile: reconstructed the
`ask.rs` threshold ladder (fixing a defeated safety-cap net, a dropped 50 %
elide branch, and a persistence-flag regression), fixed a **stream.rs
self-deadlock** (held the `sessions` lock across `spawn_compaction`, which
re-locks), corrected the `background.rs` idempotency guard (compared against
the whole snapshot's last turn → never fired), converted lock sites to
`.unwrap_or_log()`, gated the narrative call on `narrative_enabled`, and wrote
the 4 missing tests (executor shipped 3/7). Also fixed a pre-existing recall
test HOME-isolation gap the new tests exposed. Gates green (900 unit + 27
integration, 3× deterministic). Every epoch/compaction-path phase (03, 05a,
05b, 06, 07, 08) has now needed architect takeover — the compaction-path
rewire shape reliably defeats this executor (04, a pure archive add, was the
lone `approved_first_try` in this stretch).

**M4 phase-07 — recall-context is `done`** (2026-07-14, escalated → architect
takeover after 2 no-progress stalls). New `recall_context` tool over the phase-04
archive (query/range, char-safe excerpts, masked+truncated). The new rexyMCP
`NoProgressStall` governor **validated in the wild** — caught both stalls (20,
then 40 turns) instead of 167-529-turn runaways; threshold raised 20→40 for this
project. Executor wrote a near-complete impl on the 2nd run; takeover finished §3
wording + sre.toml, fixed the should_emit arm, a build_excerpt byte/char bug, and
the recall tests' HOME isolation. Gates green (893 unit + 27 integration).

**M4 phase-06 — ledger-rollups is `done`** (2026-07-14, escalated → architect
takeover). Executor stopped by the human (`rexymcp stop`) at 167 turns
verify-looping — its implementation was complete and compiled
(`maybe_rollup`/`uncovered_epochs`/`EpochTally::merge`/`summarize_once` extract/
ledger render/`rollup_after` config); takeover fixed test-only defects (HOME-leak
→ RAII guard, wrong turn_end assertion, poison-resilient `TEST_HOME_LOCK`, clippy
nits) and restored a README the executor's edit tool corrupted. Gates green
(883 unit + 27 integration). **3rd consecutive human-stopped verify-loop on an
epoch phase** — reinforces filed FR-2.

**M4 phase-05b — epoch-head is `done`** (2026-07-14, escalated → architect
takeover). Executor was **stopped by the human (`rexymcp stop`) after 529 turns
of verify-looping**; its `epochs.rs` (`compact_with_epochs` regenerated head +
`render_context_block`) and `ask.rs` (should_digest epoch-build rewire) were
correct, but `digest.rs` was left garbled mid-edit. Architect reconstructed
digest.rs from HEAD + reapplied the intended deletions (retired
`build_session_digest`/`compact_with_digest`/`tally_events`/`scan_artifacts`;
kept the narrative summarizer + budget planner; keep-newest narrative), fixed
executor clippy/test bugs. Gates green (874 unit + 27 integration).

**M4 phase-05a — epoch-persistence is `done`** (2026-07-14, escalated → minimal
architect takeover). Executor authored `src/daemon/context/epochs.rs` (+514,
all functions + tests, correct) but verify-looped (`IdenticalToolCallRepetition`,
6 bash calls) on a 1-line test-fixture bug (event ts 15:00 vs window
`[00:00,01:00)`). Notably it did **not** git-revert this time — the split's
additive 05a left the code intact for a trivial takeover (fixed the window; gates
green, 884 unit + 27 integration). digest.rs untouched — additive contract held.

**Phase-05 was re-split (2026-07-14, PE decision) into 05a + 05b.** At ~500
lines it sat at the one-session limit and it deletes/replaces the phase-03
compaction path — the exact digest-heavy shape the executor git-thrashed on
twice. **05a** (`phase-05a-epoch-persistence.md`) is purely additive:
`context/epochs.rs` types + append-only persistence + span-windowed
`tally_span`/`scan_artifacts_span`, deleting/rewiring nothing (build stays green
throughout). **05b** (`phase-05b-epoch-head.md`) does the risky rewire:
`compact_with_epochs` regenerated head, `render_context_block`, keep-newest
narrative, and retirement of `compact_with_digest`/`build_session_digest`. 05b's
Current-state quotes the phase-03 takeover `should_digest` block verbatim so the
executor rewires in place. The old `phase-05-epoch-records.md` is now a redirect
stub.

**M4 phase-04 — append-only-archive is `done`** (2026-07-14, approved_first_try,
commit `0c02961`). Archive folded into `append_session_message` (all 7 callers
automatic, archive-first ordering to avoid seed-duplicate); honest elision
placeholders; `sweep_session_archives` retention.

**M4 phase-03 — budget-compaction is `done`** (2026-07-14, escalated → architect
takeover). Executor hard_failed after 352 turns: it wrote the `digest.rs` core
then **reverted it via `git checkout`/`git stash`** (despite a runtime guard),
leaving a non-compiling tree with only the plumbing. Architect implemented
`digest.rs` (§2 budget planner + `raw_budget_cut`, §4 `synthesized_tail_start` +
`repair_tail_head`, §5 graduated UTF-8-safe elision, 3-arg pure-cutter
`compact_with_digest`), fixed 3 executor plumbing deviations (`validate_compaction`
fallback, hardcoded `token_scale=1.0` → real per-session scale, dead
`_history_pct`), and verified E2E (real binary emits the `[compaction]` fallback
warning). Gates green (875 unit + 27 integration). 2nd occurrence of the Qwen
git-thrash pathology (phase-01 was 1st) — one more warrants a WORKFLOW fold.

**M4 phase-02 — token-estimation is `done`** (2026-07-14, approved_after_1).
Delivered `src/daemon/context/estimate.rs` (deterministic per-message estimate
`chars/4 + 8 + 12·items`, `estimate_history_tokens`, EMA `update_token_scale`
clamped to [0.5, 4.0]), `token_scale: f64` on `SessionEntry` (all 7 construction
sites), calibration at both `stream.rs` write-back sites, and the post-restart
blind-spot fix in `server/ask.rs`. Bounced once (bug-02-1, major
`masked_diagnostic`): the first run computed `effective_prompt_tokens` but bound
it to a `_`-suppressed variable and never consumed it, so the blind-spot fix was
a no-op that passed clippy. Fix `cb92cd3` wired it into `token_pct` +
`PromptCtx`; bug `verified`, gates green (867 unit + 27 integration). Consumer is
phase 03.

**M4 phase-01 — events-rotation is `done`** (2026-07-09, escalated → session
takeover after 1 bounce; committed `3d74880`). Executor implemented the phase +
bug fixes but looped on the bug-01-3 test verification (120+ turns grepping test
stdout); the architect finished it in the main loop — extracted
`aggregate_over_range()` for a real cost-sort test (bug-01-1), corrected the
search-tail test query (bug-01-3), and ran both real-binary E2E scenarios
(bug-01-2). All three bugs `verified`; gates green (862 unit + 27 integration).

**M4 — Context Management Overhaul is scoped** (2026-07-07, PE sign-off). The
design is `docs/design/context-management.md` (failure catalog D1–D15 + target
architecture); the milestone README with all ten phase rows is
`docs/dev/milestones/M4-context-management/README.md`. All ten phase docs were
drafted at kick-off by explicit PE request — **re-verify each doc's Current
state section against the working tree before dispatching it** (earlier phases
move its anchors; each doc carries a Pre-flight step for this).

Phase order: 01 events-rotation → 02 token-estimation → 03 budget-compaction →
04 append-only-archive → 05 epoch-records → 06 ledger-rollups →
07 recall-context → 08 async-compaction → 09 session-meta-persistence →
10 ghost-and-memory.

---

**M3 — Polish & Maintenance is complete** (2026-06-28; all 10 phases `done`,
all `approved_first_try`, zero bounces, zero bug reports). Retrospective in
`docs/dev/milestones/M3-polish-maintenance/README.md` § Retrospective. All seven
M3 exit criteria met; no STANDARDS.md / WORKFLOW.md folds this milestone (M3 was
all maintenance-shaped work that confirmed existing folds rather than revealing
new patterns). The two M3 survey holdovers (error-result/response-builder
helper ~74 sites; executor approval-gate extraction) remain deferred.

---

**M3 phase-09 — consolidate-loop-ctx is `done`** (approved_first_try, 2026-06-28).
Consolidated the two remaining high-arity orchestration signatures via borrow-structs
(`AskRequest`/`AskContext` for `handle_ask`, `ConversationLoopCtx` for
`run_conversation_loop`), deleting the last two `#[allow(clippy::too_many_arguments)]`
suppressions + two `TODO(M2)` markers — clearing the "7 `TODO(M2)` markers resolved" exit
criterion. Executor commit `7edabde`; review approval `67a4d78`.

**M3 phase-08 — help-and-truncation is `done`** (approved_first_try, 2026-06-28). Added
ellipsis truncation markers on silent truncation (status bar / panel / committed text) and
completed the `/help` text (aliases, document redirect + tool-output cap). Executor commit
`66b6654`.

**M3 phase-07 — split-webhook is `done`** (approved_first_try, 2026-06-28). Split the
1210-line `webhook.rs` grab-bag into a `webhook/` directory module with three cohesive
submodules (`parse` / `process` / `server`) via the M2 C5-split idiom; glob re-exports keep
every `crate::webhook::<name>` path resolving, zero consumer edits. Only non-move edit:
`AlertStatus::as_str` `fn` → `pub(crate) fn`. Executor commit `d8aba17`; review approval `e125eae`.

**M3 phase-06 — error-hardening is `done`** (approved_first_try, 2026-06-28). Three
behavior-preserving hardening edits: `memory_prompt.rs` double-lookup → single Entry-API
expression; four `ai/mod.rs` circuit-breaker lock sites → documented `.unwrap_or_log()`
invariant (ERROR-on-poison logging); five `daemon/scheduled.rs` swallowed `notify_tx` sends →
`log::debug!` on dropped receiver. Executor commit `e7a1658`; review approval `b040651`.

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
