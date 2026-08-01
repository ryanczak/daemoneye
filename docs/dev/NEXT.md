# NEXT

**Active phase: none.** Draft the next one with `/rexymcp:architect next` —
phase 08 (fts5-search) is next, and it is the phase that finally makes the
milestone's headline capability real.

**Phases 01–07 and 10 are `done`; only 08 and 09 remain.** The index is built
(06), maintained on every add/update/delete with a reconciliation rebuild (07),
and the runtime tree and docs no longer lie (10). `fts5_search()` is still a stub
by design — 08 wires it.

## Phase 08 must resolve three things, two of them load-bearing

1. **A fresh install has an empty index — this is the big one.** Seeded memories
   are written by `seed_memory_inner` with a direct `fs::write`
   (`src/config/seeds.rs:80`), which bypasses `add_memory` and therefore the
   index hook. **None of the seven built-in knowledge memories is ever indexed**,
   so on a fresh install the index has zero rows. Wire `fts5_search()` without
   fixing this and M7 ships a search feature that cannot find its own seed data —
   the exact recall failure the milestone exists to remove. `reconcile_index()`
   is the fix and is precisely the production caller phase 07 deferred. **Decide
   where it is called**: daemon startup, lazily on first search, or an operator
   `reindex` command.
2. **Quote every user query before it reaches `MATCH`.** A bare `-` or `:` is
   FTS5 query syntax, so `MATCH 'runtime-layout'` raises *"no such column:
   layout"* — and memory keys are kebab-case, so this fires on ordinary input.
   Verified against `sqlite3 3.53.4`; double-quoting fixes it. Pin a negative
   test on a hyphenated query.
3. **Deciding (1) may let the two item-level `#[allow(dead_code)]` on
   `reconcile_index` / `ReconcileReport` come off.** If 08 gives them a
   production caller, delete the attributes — `grep -c 'allow(dead_code)'
   src/memory/index.rs` should go to 0. If 08 defers again, say so explicitly in
   the spec rather than leaving it implied.

**Write the lint-gate decision into the spec explicitly.** This trap has now
cost two phases: 06 bounced on it, and 07 hard-failed for 90 turns on a spec
that decided it *inconsistently* (delete the allow / leave the function
uncalled — both, impossibly). See phase 07's verdict § Calibration.

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
