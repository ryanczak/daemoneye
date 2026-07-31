# NEXT

**Active phase: M7 phase-01 — dependency-currency** (`todo`, drafted 2026-07-31).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-01-dependency-currency.md`

Dispatch with `/rexymcp:dispatch phase-01`.

The migration was verified end-to-end in a throwaway copy of `HEAD` before the
spec was written: build, clippy and the full suite (991 + 30 + 8) were green with
exactly the version requirements the spec pins. The spec says so, so the executor
applies a known-good change rather than exploring one.

## What M7 covers

One capability and one maintenance axis:

- **Working memory search.** `fts5_search()` is an eight-line stub returning an
  empty `Vec`, and it is one of three candidate sources in memory recall — so a
  memory whose *text* matches what the user said surfaces only if its *tags*
  happen to overlap. Degraded silently today.
- **Maintenance:** dependency currency, the path-audit gate's blindness to fenced
  code blocks, a generated runtime-layout tree, a bug-tracker truth gate, and the
  four test sleeps `STANDARDS.md` §3.3 forbids.

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
