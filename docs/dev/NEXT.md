# NEXT

**Active phase: M7 phase-05 — generated-runtime-tree** (`todo`, drafted
2026-07-31).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-05-generated-runtime-tree.md`

Dispatch with `/rexymcp:dispatch phase-05`.

**The design fork was settled before drafting** (PE, 2026-07-31): render the
tree from a table in Rust and assert equality against the shipped asset, rather
than a drift-test-only approach or a 05a/05b split. The tree carries data
`POLICY_TABLE` does not have — files, the `memory/{session,knowledge,incident}`
split, and purpose annotations — so it gets its own table with a cross-check
test between the two. Phase 06 will need entries in **both** tables; the
cross-check makes the second impossible to forget, which is the honest claim
(not "the tree updates itself").

**Both load-bearing spec claims were prototyped against the real asset before
the spec was written**, so the executor's job is mechanical:

- The pre-injected `RUNTIME_TREE` literal plus the format rules (2-space indent,
  `←` padded to column 29, 7 blank separators) render the shipped block
  **byte-for-byte** — verified, `MATCH`.
- All **15** `POLICY_TABLE` paths match a tree path under the segment-wise
  wildcard rule (`agents/*/mailbox` ↔ `agents/<name>/mailbox`) — verified, no
  misses.

No `build.rs`: the asset stays a checked-in file behind `include_str!`. The
first acceptance criterion is that the asset is **byte-for-byte unchanged** —
if the renderer disagrees, the renderer is wrong.

Phases 01–04 are `done`. 01–03 approved_first_try; **04 approved_after_1**
(`bugs/bug-04-1.md`, minor, `scope_deviation` — the fence rewrite dropped the
`on_line` guard from the *non-fence* branch; round 2 restored it as a `closed`
flag and pinned it). M6 open question 5 Part A is resolved.

**Two things phase 05 should carry forward:**

1. **`tests/isolation.rs` is intermittently flaky.** A full `cargo test` failed
   once at review in `hooks_land_on_private_server`, then went green across 5
   full-suite and 12 isolation-only runs. Ruled out as phase-04's doing
   (`path_audit`'s only caller is `src/cli/commands/audit_prompts.rs`;
   `tests/isolation.rs` never references the audit) and as phase-03's
   (`4472293` does not touch that file). The test spawns a real daemon and tmux
   server. Pre-existing; worth its own phase if it recurs.
2. **The fence toggle is a flip-flop, not a nesting parser.** `in_fence` inverts
   on any line starting with ` ``` `, so a nested fence inside a fence — which
   phase-04's own Update Log now contains — mis-tracks. Harmless while
   `audit-prompts` only scans installed assets, but it bites the moment the
   audit is pointed at `docs/`.

**This phase resolves M6 open question 5**, which was deferred because a naive
"contains a slash" rule false-positives on `/clear` and shebangs. Both candidate
rules were prototyped against the real assets before the spec was written:

- **Naive** (every prefix-matching token in a fence): 11 extractions, **4 false
  `Unknown` findings** — `audit-prompts` would exit 1 on a clean tree.
- **Narrow** (only tokens whose normalised form is multi-segment): 1 extraction,
  **0 false findings**, audit stays at exit 0 — and it still catches a fenced
  `var/index/memory.db`, the phantom that slipped through M6.

The false positives are context-loss, not drift: `agent-runtime-layout.md`'s tree
is indentation-relative, so a bare `prompts/` means `etc/prompts/`.

The E2E block carries phase-03's post-mortem rules: **no heredocs**, and every
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
- 991 lib + 30 integration (2 ignored) + 8 isolation (1 ignored); clippy clean;
  20 consecutive `cargo test --lib` runs clean.
- Working tree clean. No daemon running; no tmux server running.
- No live bugs: the five bug docs still marked `open` across M2/M4 were each
  verified fixed against the code — M7 phase 02 closes them and lands a gate so
  the tracker cannot drift again.
