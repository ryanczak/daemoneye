# NEXT

**Active phase: M7 phase-07 — fts5-write-path** (`todo`, drafted 2026-08-01,
already dispatch-ready).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-07-fts5-write-path.md`

Dispatch with `/rexymcp:dispatch phase-07`.

**Phase 10 is `done`** (approved_first_try, 2026-08-01) — dispatched out of order
at PE request. `memory/incident` → `incidents` is fixed at all three sites
including the live stamping bug, the gate gap is closed, and the two false
`CLAUDE.md` rows are corrected. Detail below, kept because 08 and 09 both touch
the same documents.

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

## Phase 07 — what it is

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

## Also outstanding

- **The FTS5 `MATCH` quoting gotcha** — a bare `-` or `:` is query syntax, so
  `MATCH 'runtime-layout'` raises *"no such column: layout"* and memory keys are
  kebab-case. **Phase 08 owns it.**
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

**Phases 01–06 and 10 are `done`; 07 is drafted, 08–09 named only.** Verdicts:
01–03, 05 and 10 approved_first_try; **04 approved_after_1**
(`bugs/bug-04-1.md`, minor, `scope_deviation`); **06 approved_after_1**
(`bugs/bug-06-1.md`, minor, `spec_bug` — the architect's under-specification,
not the model's).

**Prototyping the spec before writing it is now 4 for 4** (04, 05, 06, 10). Each
had its load-bearing facts executed against the real system first — the two
candidate extraction rules for 04, the renderer and tree data for 05, the FTS5
DDL against `sqlite3 3.53.4` for 06, and for 10 the exact tree lines, the
eager/lazy split from a real `daemoneye setup`, and every `CLAUDE.md` claim
checked one at a time. All four landed clean on the prototyped parts; 06's single
bounce was in the one area that was *not* prototyped (the test idiom). Worth
stating plainly at milestone close.

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
- **1015 lib + 30 integration (2 ignored) + 8 isolation (1 ignored) + 6
  bug_tracker**; clippy clean; `cargo fmt --all --check` clean. Independently
  verified at phase-10 review. (M6 closed at a 991-lib baseline; +9 phase 04,
  +1 bug-04-1, +5 phase 05, +6 phase 06, +2 phase 10. The residual +1 predates
  phase 04 — its own run already reported a 992 starting point — and has not
  been traced to a specific phase.)
- Working tree clean. No daemon running; no tmux server running.
- **No open bugs.** `bug-04-1` and `bug-06-1` are both `verified`; the five stale
  M2/M4 docs were closed by phase 02, which also landed the `bug_tracker` gate so
  an `open` bug on a `done` phase now fails the suite.
