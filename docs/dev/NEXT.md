# NEXT

**Active phase: M7 phase-07 — fts5-write-path** (`todo`, drafted 2026-08-01).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-07-fts5-write-path.md`

Dispatch with `/rexymcp:dispatch phase-07`.

Phase 06's foundation is in — `rusqlite 0.40.1` (`bundled`), the schema at
`var/index/memory.db`, the path in all four gates. Phase 07 makes it live:
`add_memory` / `update_memory` / `delete_memory` maintain the index, plus a
`reconcile_index()` rebuild. `fts5_search()` stays a stub until 08.

**All three of phase 06's carried requirements are folded into the 07 spec**:
deleting `#![allow(dead_code)]` is task 1 and an acceptance criterion; the
lint-gate question is answered (nothing 07 lands stays unused, so the attribute
simply goes); and every API is named concretely rather than gestured at —
`tempfile::tempdir()`, `super::parse_memory_frontmatter`, `list_agents()`,
`log::warn!`.

**Five facts verified against the real code before the spec was written**, so
the executor is not guessing:

- **FTS5 has no upsert** — `ON CONFLICT` errors with *"UPSERT not implemented
  for virtual table"*. Update must be scoped `DELETE` then `INSERT`; verified
  that a scoped delete keys correctly on `namespace` and leaves other namespaces
  intact.
- **`memory::index` can call `memory`'s private `parse_memory_frontmatter`** —
  `pub mod index;` is declared inside `src/memory.rs`, so it is a descendant
  module. Compiled and run to confirm, so the spec says "use
  `super::parse_memory_frontmatter`" instead of inviting a second parser or a
  needless `pub`.
- **`MemoryInfo` carries no body**, so the reconciler cannot be built on
  `list_memories_with_tags` alone — it must read and parse each file.
- **Namespaces must be enumerated** — `"global"` plus `list_agents()` names.
- **The incident directory is `incidents`, plural** (see below).

**The load-bearing test is `reconcile_after_incremental_writes_is_a_no_op`** — a
full rebuild after a mixed add/update/delete sequence must find the same row
count. It asserts that two independent paths to the same state agree rather than
a hand-computed number, which is the milestone's "verified by a reconciliation
test rather than by construction order" criterion expressed as code.

## A phase is missing from the table

Two runtime-tree defects surfaced while drafting 06 and 07. Both were held out of
those phases deliberately to keep them focused, and **both still need a phase**
before M7 closes. Recorded with full detail in the milestone README §
"Runtime-tree defects found mid-milestone":

1. **`memory/incident/` does not exist — the real directory is
   `memory/incidents/`.** `dir_name()` returns the plural, `canonical_name()` the
   singular, and `RUNTIME_TREE` plus the shipped asset document the singular.
   Verified empirically: after `daemoneye setup`, `memory/` holds only
   `knowledge` and `session`, and `incidents/` appears lazily on first write. The
   agent-facing knowledge memory is telling the AI a path that cannot exist —
   exactly the defect class M6 item 5 was about. Phase 05's gates missed it
   because `POLICY_TABLE` carries only `memory`, not the per-category paths.
2. **`agents/*/memory/` is in neither `POLICY_TABLE` nor `RUNTIME_TREE`.**

They share a fix shape and one asset regeneration, so they want one phase, not
two.

## Also outstanding

- **`CLAUDE.md` overstates `add_memory`/`update_memory`** — it claims they
  "enforce size cap, fcntl lock, masking, index sync (G1)". Verified at drafting:
  they do none of those. Phase 09 owns the correction; phase 07 is explicitly
  forbidden from touching `CLAUDE.md`.
- **The FTS5 `MATCH` quoting gotcha** — a bare `-` or `:` is query syntax, so
  `MATCH 'runtime-layout'` raises *"no such column: layout"* and memory keys are
  kebab-case. **Phase 08 owns it.**
- **Schema-version bumps are free** — `ensure_schema` drops and recreates on a
  `user_version` mismatch, pinned by `stale_schema_version_is_recreated`. If a
  later phase needs a column, bump `SCHEMA_VERSION`; do not write a migration.

**M6 open question 5 is fully resolved** — Part A (phase 04, the audit reads
fenced blocks) and Part B (phase 05, the tree renders from Rust data with a
byte-for-byte equality test against the shipped asset).

**Prototyping the spec before writing it has now paid off three times** (04, 05,
06). For 06 the schema was verified against `sqlite3 3.53.4` before a line of
spec was written, and the asset lines were computed with the phase-05 renderer —
neither the DDL nor the tree edit needed a correction. Both of 06's problems came
from the parts of the spec that were *not* prototyped: the lint gate and the test
idiom. Worth remembering when scoping 07.

**Four things still carried forward:**

1. **`tree_block_of` has a loose error contract** — an unterminated fence returns
   `Some` where phase 05's spec said `None`. Reviewed as a documented nit rather
   than a bounce (no reachable consequence; every corruption path still fails
   loudly). Deliberately *not* folded into phase 06, to keep that phase's scope
   clean — worth doing whenever that file's neighbours are next open.
2. **`agents/*/memory/` is in neither `POLICY_TABLE` nor `RUNTIME_TREE`.** Found
   while drafting phase 06: `memory_dir_for_namespace()` (`src/memory.rs:240`)
   creates `agents/<ns>/memory/<category>/` for non-global namespaces and no
   table lists it. A real pre-existing gap, explicitly out of scope for phase 06
   because fixing it drags in an unrelated asset change. Needs its own phase.
3. **`tests/isolation.rs` is flaky — now twice, which makes it a trend.**
   Occurrence 1: phase-04 review, `hooks_land_on_private_server`, then green
   across 5 full-suite and 12 isolation-only runs. Occurrence 2: phase-06's own
   run, `stub_returns_canned_response_via_make_client`, an `AddrInUse` port-bind
   race, green on re-run. **Two different tests in the same file, both binding
   ports / spawning real daemons** — that is a shared root cause, not two
   coincidences. Ruled out as phase-03/04/05/06 fallout each time. Per
   `WORKFLOW.md` § Calibration, one is data and two is a trend: this now warrants
   its own phase (ephemeral-port allocation, or serialising the port-binding
   tests) rather than another carry-forward line.
4. **The phase-04 fence toggle is a flip-flop, not a nesting parser.** `in_fence`
   inverts on any line starting with ` ``` `, so a nested fence inside a fence
   mis-tracks. Harmless while `audit-prompts` only scans installed assets, but it
   bites the moment the audit is pointed at `docs/`.

Phases 01–05 are `done`. 01–03 and **05** approved_first_try; **04
approved_after_1** (`bugs/bug-04-1.md`, minor, `scope_deviation` — the fence
rewrite dropped the `on_line` guard from the *non-fence* branch; round 2 restored
it as a `closed` flag and pinned it).

**Both 04 and 05 were prototyped before their specs were written**, and both
landed clean as a result. For 04 the architect ran the two candidate extraction
rules against the real assets (naive: 11 extractions and 4 false `Unknown`
findings, which would have made `audit-prompts` exit 1 on a clean tree; narrow
multi-segment: 1 extraction, 0 false findings). For 05 the architect built the
renderer and the `RUNTIME_TREE` data as a throwaway prototype and confirmed
byte-for-byte reproduction plus all 15 policy-path matches before writing a line
of spec. On a transcription-shaped task a single wrong space would have made the
primary acceptance criterion unsatisfiable — worth doing again for phase 06's
schema work.

E2E blocks carry phase-03's post-mortem rules: **no heredocs**, and every
tree-walking command wrapped in `timeout`.

## The phase-06 dependency decision is settled

**`rusqlite` with the `bundled` feature** (PE, 2026-07-31). Recorded with its
rationale and the empirical verification in the milestone README's Notes. Phase 06
is authorized to add it; phase 06's spec must carry that authorization explicitly,
since `STANDARDS.md` §2.6 makes an unauthorized dependency an always-blocker.

Verified rather than assumed: `bundled` alone yields `ENABLE_FTS5` (no
`bundled-full`), latest stable is `0.40.1` bundling SQLite 3.53.2, and the
`ffi-sqlite-wasm-rs` default feature is target-gated to wasm and compiles nothing
natively.

## Where the tree stands

- M6 closed: 13 phase docs `done`, retrospective in its README.
- **1007 lib + 30 integration (2 ignored) + 8 isolation (1 ignored) + 6
  bug_tracker**; clippy clean; `cargo fmt --all --check` clean. Independently
  verified at phase-05 review. (M6 closed at a 991-lib baseline; phase 04 added
  9 plus 1 for bug-04-1, phase 05 added 5. The residual +1 predates phase 04 —
  phase-04's own run already reported a 992 starting point — and has not been
  traced to a specific phase.)
- Working tree clean. No daemon running; no tmux server running.
- **No open bugs.** `bug-04-1` is `verified`; the five stale M2/M4 docs were
  closed by phase 02, which also landed the `bug_tracker` gate so an `open` bug
  on a `done` phase now fails the suite.
