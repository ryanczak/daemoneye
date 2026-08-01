# NEXT

**Active phase: M7 phase-06 — fts5-index-schema** (`todo`, drafted 2026-08-01).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-06-fts5-index-schema.md`

Dispatch with `/rexymcp:dispatch phase-06`.

This starts the FTS5 work that is the milestone's actual capability. It is
**foundation only** — `rusqlite`, the schema, the database file, and registering
`var/index/memory.db` in the four gates that each fail independently if it is
missing (`POLICY_TABLE`, `RUNTIME_TREE` + its asset, and the path-audit
`INVENTORY`). `fts5_search()` stays a stub, pinned by an acceptance criterion;
phase 07 owns the write path, phase 08 the query path.

**M6 open question 5 is fully resolved** — Part A (phase 04, the audit reads
fenced blocks) and Part B (phase 05, the tree renders from Rust data with a
byte-for-byte equality test against the shipped asset).

**Prototyped before the spec was written**, as with 04 and 05. Verified against
`sqlite3 3.53.4` rather than assumed: the DDL is accepted, `PRAGMA user_version`
round-trips, `porter` stemming makes `MATCH 'run'` find "running", an `UNINDEXED`
column is filterable but not searchable, and `bm25()` is callable. Also confirmed
`rusqlite-0.40.1` and `libsqlite3-sys-0.38.1` are already in the local cargo
cache and `cc (GCC) 16.1.1` is installed — the bundled build needs no network.
The exact three asset lines the tree change requires were computed with the
phase-05 renderer and pasted into the spec.

**One gotcha recorded now because it is easy to lose:** in an FTS5 `MATCH`
expression a bare `-` or `:` is query syntax, not text — `MATCH 'runtime-layout'`
raises *"no such column: layout"*. Memory keys are kebab-case, so every
user-supplied query must be double-quoted. Phase 06 builds no query path;
**phase 08 owns this.**

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
3. **`tests/isolation.rs` is intermittently flaky** — a full `cargo test` failed
   once during phase-04 review in `hooks_land_on_private_server`, then went green
   across 5 full-suite and 12 isolation-only runs. Ruled out as phase-04's doing
   and as phase-03's. The test spawns a real daemon and tmux server. Pre-existing;
   worth its own phase if it recurs. Do not let it be mistaken for FTS5 fallout.
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
