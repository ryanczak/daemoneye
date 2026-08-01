# NEXT

**Active phase: none.** Draft the next one with `/rexymcp:architect next` —
phase 06 (fts5-index-schema) is next in the table, and it starts the FTS5 work
that is the milestone's actual capability.

**M6 open question 5 is now fully resolved** — Part A (phase 04, the audit reads
fenced blocks) and Part B (phase 05, the runtime tree renders from Rust data with
a byte-for-byte equality test against the shipped asset).

**Four things phase 06 inherits.** It adds `var/index/memory.db`, which touches
most of them:

1. **It needs entries in *two* tables, not one.** `POLICY_TABLE`
   (`src/config/lifecycle.rs`) *and* `RUNTIME_TREE`
   (`src/config/runtime_tree.rs`). The README's "the tree updates itself" framing
   is optimistic — the tree carries files and purpose annotations the policy
   table does not have, so it has its own table. What phase 05 guarantees is that
   the second edit **cannot be forgotten**: `every_policy_path_appears_in_tree`
   fails until it exists. Also add the path-audit `INVENTORY` entry, or the
   phase-04 gate will report it `Unknown`.
2. **`tree_block_of` has a loose error contract** — an unterminated fence returns
   `Some` where the spec said `None`. Reviewed as a documented nit rather than a
   bounce (no reachable consequence; every corruption path still fails loudly).
   Phase 06 is already in this file — tighten it there with a `closed` flag, the
   shape phase-04 landed in `extract_path_literals`. One line and a test.
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
