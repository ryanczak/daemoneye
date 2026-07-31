# NEXT

**Active phase: M7 phase-02 — bug-tracker-truth** (`todo`, drafted 2026-07-31).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-02-bug-tracker-truth.md`

Dispatch with `/rexymcp:dispatch phase-02`.

Phase 01 (dependency-currency) is `done`, approved_first_try.

The gate's algorithm was validated against the real tree before the spec was
written: it produces exactly five findings — the known-stale M2/M4 docs — and no
false positives. The spec pre-injects the four parsing traps that would otherwise
cost round-trips (status lines carry trailing prose; three phase docs have a
second `Status:` line inside an Update Log entry; bug filenames use two
conventions; phase docs match by prefix), plus the fact that `tests/` sees only
dev-dependencies, so `regex` is unavailable there.

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
