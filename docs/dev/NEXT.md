# NEXT

**Active phase: M7 phase-04 — path-audit-fenced-blocks** (`todo`, drafted 2026-07-31).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-04-path-audit-fenced-blocks.md`

Dispatch with `/rexymcp:dispatch phase-04`.

Phases 01-03 are `done`, all approved_first_try.

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
