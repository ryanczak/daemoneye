# NEXT

**Active phase: M7 phase-03 — test-sleep-removal** (`todo`, drafted 2026-07-31).

Doc: `docs/dev/milestones/M7-memory-search-and-maintenance/phase-03-test-sleep-removal.md`

Dispatch with `/rexymcp:dispatch phase-03`.

Phases 01 and 02 are `done`, both approved_first_try.

**The milestone README's "four sleep sites" was wrong and has been corrected.**
Re-scanning by enclosing-function attributes rather than by text grep found
**three** live sites — including a 3-second wall-clock wait in
`liveness_is_unresponsive_when_peer_never_replies` that the original count
missed entirely — while five of the originally-listed sites turned out to be
already compliant inside `#[ignore]`d tests.

All three fixes were applied, run and reverted before the spec was written:
`start_paused = true` takes the 3 s test to `0.00s`, and both changed tests held
green across 15 consecutive runs each.

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
