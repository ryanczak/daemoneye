# NEXT

**Active phase: M7 phase-08 — fts5-search** (`in-progress`, drafted 2026-08-01,
**bounced at review 2026-08-01** — see `bugs/bug-08-1.md`).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-08-fts5-search.md`

Re-dispatch with `/rexymcp:dispatch phase-08`.

**Round 1 bounced on test strength, not correctness** (`missing_spec_test`). The
implementation is right — 1031/30/8/6, clippy clean, both `allow(dead_code)`
gone — but each of the phase's two central mechanisms survives deletion with the
suite green: removing `ORDER BY bm25` leaves `search_ranks_better_match_first`
passing (its fixture inserts strong-then-weak, so rowid order equals rank order),
and replacing `build_match_expr` with naive whole-query phrase quoting passes all
22 tests (every search test uses a single-token query). Both fixes are proven in
the bug doc. The phase doc's own claim that two named tests would catch the
quoting mode was wrong and has been corrected in place — the architect's error.

**This is the milestone's headline capability** — BM25-ranked memory search, and
M7's first exit criterion. After it, only phase 09 (the doc correction) remains.

**Every fact in the spec was executed against SQLite 3.53.4 before drafting.**
That is deliberate: the rule that fell out of phases 06 and 07 is *do not assert
a fact about the system in a spec unless it was executed*, and this is the first
spec written under it end to end.

What the prototyping found, and why the spec looks the way it does:

- **`bm25()` is negative and more-negative is better** (`-0.000001812` vs
  `-0.000000798` for the same term), so `ORDER BY bm25(memories)` ascending is
  best-first. Easy to get backwards.
- **Double-quoting the *whole* query makes search useless.** Quoting turns the
  expression into a phrase match, and the caller passes the **entire user turn**
  (`ftsearch_memories(user_turn, 10, …)`, `memory_prompt.rs:73`). Executed:
  `MATCH '"how do I tune shared_buffers for postgres?"'` → **0 rows**, against a
  memory that is literally about that. Per-term quoting joined with `OR` → **1
  row**. The spec pins per-term construction with a worked example.
- **`OR` lets noise in and `bm25` handles it** — a relevant doc scored
  `-0.000003162` against a stopword-only match at `-0.000001903`. So the spec
  asks for no stopword list; ranking is the mechanism, and the ranking test must
  assert **order**, not membership.
- **A fresh install reconciles to exactly 9 rows** (7 knowledge + 2 session).
  Measured by running `reconcile_index()` in a seeded temp `HOME`, not counted by
  hand.
- **The dynamic `IN (…)` clause with `params_from_iter` was compiled and run**
  before being pasted into the spec; it returned `[("global","k",-1e-6)]`.

**Three design decisions settled in the spec rather than left open:**

1. **`fts5_search` gains a `namespaces` parameter** and returns
   `(namespace, key, score)`. Today it filters nothing, so `limit` is applied
   before the caller drops out-of-namespace hits — asking for 10 can yield 3 —
   and `m.key == key` ignores namespace even though phase 07 proved the same key
   can exist in two.
2. **Reconcile-on-empty, triggered by row count, not a `Once` latch.** This is
   the fix for the empty-fresh-install gap phase 07 recorded. A process-global
   latch would fire in whichever test ran first and leave the rest unreconciled —
   the same trap that rules out a cached `Connection`.
3. **Both `#[allow(dead_code)]` come off** — task 2 gives `reconcile_index()` its
   first production caller. The spec states plainly that nothing is left
   deliberately unused, which is the lint-gate decision that phases 06 and 07
   each got wrong in a different way.

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
