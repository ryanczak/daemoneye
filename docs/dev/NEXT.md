# NEXT

**Phase 09 is `done` (approved_first_try). All ten M7 phases are now `done`.**

**The milestone is at its boundary — a human gate.** Run `/rexymcp:architect` to
close it: write the retrospective, fold the calibration lessons, and set this
file's active phase to "none". **That has deliberately not been done here** —
the review step does not write retrospectives or close milestones, so this
pointer is left mid-state on purpose rather than by oversight.

## Phase 09 — what it is

Five prose sites across `docs/architecture.md` and `CLAUDE.md`, plus a tripwire.
**Three of the five became wrong when phase 08 landed; two were never true**, and
each was checked against the code before drafting:

- **There is no grep fallback for recall, and there never was.**
  `grep -c "crate::search" src/daemon/memory_prompt.rs` returns **0**. Recall
  merges tag overlap, one-hop `relates_to`, and FTS5 hits. `src/search.rs` backs
  the `search_repository` *tool* — its only caller is
  `src/daemon/executor/knowledge/memory.rs:235`. Two sites claim otherwise.
- **The "G2 schema" does not exist.** All four of `volatility`,
  `usefulness_score`, `last_verified`, `verified_by` return **no files** under
  `src/`. Phase 10 removed this claim from `CLAUDE.md`; architecture.md § 3 is
  the surviving copy.
- **§ 5 carries two stale counts** — "nine phases named, none drafted" (it is ten,
  nine done) and "four test sleeps" (phase 03 corrected it to three).
- **The stub note itself** — architecture.md § 5's "currently a **stub**"
  paragraph, whose entire purpose was to be deleted by this phase.

**The tripwire is `tests/doc_truth.rs`**, following the `tests/bug_tracker.rs`
idiom: a table of four retired phrases that must not reappear. The spec records
their pre-edit `grep -c` counts (2, 1, 1, 1) and **requires a reinsertion red
run**, because a tripwire listing phrases that are already absent would pass
forever while guarding nothing. The spec is also explicit that this is a
tripwire for four named claims, not a general drift detector.

**Deliberately out of scope, because milestone close owns them:** rewriting § 5's
narrative into a retrospective, ticking the README's exit-criteria checkboxes,
and setting this file to "none". Phase 09 corrects prose and stops.

## After phase 09

All ten phases will be `done` and M7 hits its boundary — a human gate. Close is
`/rexymcp:architect`, which writes the retrospective and folds calibration. Three
things are already queued for it:

1. **Three consecutive phases whose only defects were architect-side** (06
   `spec_bug`, 07 `spec_bug`, 08 `missing_spec_test` + `false_completion`), against
   a perfect record for every fact that was *executed* before drafting.
2. **`tests/isolation.rs` flakiness is a trend** — two occurrences, two different
   port-binding tests, still unscheduled.
3. **`epochs.rs:618` hardcodes the category→directory mapping** instead of calling
   `dir_name()` — correct today, same latent drift phase 10 removed elsewhere.

## Phase 10 — what landed

Fixed the `memory/incident` → `incidents` defect, which **grew on contact**. The
singular had a third site that was a **live bug**, not doc drift:
`stamp_artifact_origin` (`src/session_store.rs:374`) built
`memory/incident/<name>.md`, which never exists, so **an incident memory created
inside a named session never gets its `session_origin` stamped.** No test covered
it — the existing backfill test uses a knowledge memory, which works.

**The centre of the phase was not the spelling, it was the gate gap.**
`POLICY_TABLE` carried `memory` and nothing below it, and `is_covered()` counts a
directory as covered if it is a *subdirectory of* an entry — so `memory/incidents`
was "covered" without ever being named, and phase 05's tree cross-check had
nothing per-category to compare against. That is why a non-existent path sat in
an agent-facing document through two gate-building phases. Six per-category
entries now close it, and the reviewer independently reproduced the red run:
reverting the tree node while keeping the policy entry fails
`every_policy_path_appears_in_tree` with `Policy paths not found in tree:
["memory/incidents"]`. The entries are not inert.

Also folded in: `agents/*/memory/` (in neither table), and two `CLAUDE.md` rows
that assert machinery `src/memory.rs` does not have. Verified claim by claim —
the mutators do no size-capping, locking, masking or index sync (masking and
`SESSION_MEMORY_CAP` live in `load_session_memory_block()`), 7 of the 8 listed G2
schema fields do not appear in the file at all, and there is no schema validation
or version history. The `src/memory/index.rs` row had also still said "there is
no SQLite index, no `var/index/memory.db`", which phase 06 made untrue.
**Phase 09 still owns the full index-doc rewrite** — phase 10 made only the
minimal correction, so 09 must still describe the index as built once search is
real.

## Also outstanding

- **Schema-version bumps are free** — `ensure_schema` drops and recreates on a
  `user_version` mismatch, pinned by `stale_schema_version_is_recreated`. If a
  later phase needs a column, bump `SCHEMA_VERSION`; do not write a migration.

**M6 open question 5 is fully resolved** — Part A (phase 04, the audit reads
fenced blocks) and Part B (phase 05, the tree renders from Rust data with a
byte-for-byte equality test against the shipped asset).

**Two things still carried forward:**

1. **`tests/isolation.rs` is flaky — twice, which makes it a trend.**
   Occurrence 1: phase-04 review, `hooks_land_on_private_server`. Occurrence 2:
   phase-06's own run, `stub_returns_canned_response_via_make_client`, an
   `AddrInUse` port-bind race. Both green on re-run; both ruled out as the
   phase's own doing. **Two different tests in the same file, both binding ports
   / spawning real daemons** — a shared root cause, not two coincidences. Per
   `WORKFLOW.md` § Calibration one is data and two is a trend, so this warrants
   its own phase (ephemeral-port allocation, or serialising the port-binding
   tests) rather than another carry-forward line. **Still not scheduled.**
2. **`tree_block_of` has a loose error contract** — an unterminated fence returns
   `Some` where phase 05's spec said `None`. A documented nit, not a bounce (no
   reachable consequence; every corruption path still fails loudly). Worth
   folding in whenever `src/config/runtime_tree.rs` is next open.

**Resolved by phase 10** and removed from this list: `agents/*/memory/` missing
from both tables, the `memory/incident` → `incidents` defect, and `CLAUDE.md`
overstating the memory mutators.

**Also still open, lower stakes:** the phase-04 fence toggle is a flip-flop
rather than a nesting parser (`in_fence` inverts on any ` ``` ` line), harmless
while `audit-prompts` only scans installed assets; and
`src/daemon/context/epochs.rs:618` hardcodes the category→directory mapping
instead of calling `dir_name()` — correct today, but the same latent drift phase
10 removed from `session_store.rs`.

**Phases 01–07 and 10 are `done`; 08–09 named only.** Verdicts: 01–03, 05 and 10
approved_first_try; **04 approved_after_1** (`bugs/bug-04-1.md`, minor,
`scope_deviation`); **06 approved_after_1** (`bugs/bug-06-1.md`, minor,
`spec_bug`); **07 escalated** (resume after a `hard_fail`, also `spec_bug`).
Both of the last two are charged to the architect, not the model.

**Prototyped spec facts have never needed a correction; unprototyped ones have
cost three phases.** 04, 05, 06, 07 and 10 each had their load-bearing facts
executed against the real system before drafting — candidate extraction rules,
the tree renderer, the FTS5 DDL and upsert semantics, the descendant-module
privacy rule, the eager/lazy split. **Every one of those held.** All three
failures came from the parts written from assumption: 06's test idiom
(`bug-06-1`), 07's allow/out-of-scope contradiction (a 90-turn `hard_fail`), and
07's claim that `setup` creates the `.db` file (caught by the executor). The rule
that falls out is narrow and worth stating at milestone close: **do not assert a
fact about the system in a spec unless it was executed.**

E2E blocks carry phase-03's post-mortem rules: **no heredocs**, and every
tree-walking command wrapped in `timeout`.

## The dependency decision (settled, phase 06 landed it)

**`rusqlite` with the `bundled` feature** (PE, 2026-07-31). Recorded with its
rationale and the empirical verification in the milestone README's Notes. Added
in phase 06 as `rusqlite = { version = "0.40.1", features = ["bundled"] }` — the
only dependency M7 adds.

Verified rather than assumed: `bundled` alone yields `ENABLE_FTS5` (no
`bundled-full`), latest stable is `0.40.1` bundling SQLite 3.53.2, and the
`ffi-sqlite-wasm-rs` default feature is target-gated to wasm and compiles nothing
natively.

## Where the tree stands

- M6 closed: 13 phase docs `done`, retrospective in its README.
- **1023 lib + 30 integration (2 ignored) + 8 isolation (1 ignored) + 6
  bug_tracker**; clippy clean; `cargo fmt --all --check` clean. Independently
  verified at phase-07 review. (M6 closed at a 991-lib baseline; +9 phase 04,
  +1 bug-04-1, +5 phase 05, +6 phase 06, +2 phase 10, +8 phase 07. The residual
  +1 predates phase 04 — its own run already reported a 992 starting point — and
  has not been traced to a specific phase.)
- Working tree clean. No daemon running; no tmux server running.
- **No open bugs.** `bug-04-1` and `bug-06-1` are both `verified`; the five stale
  M2/M4 docs were closed by phase 02, which also landed the `bug_tracker` gate so
  an `open` bug on a `done` phase now fails the suite.
