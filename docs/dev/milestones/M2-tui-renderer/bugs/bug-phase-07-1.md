# Bug 1 on phase-07: verbatim-move fidelity broken — 4 comment lines dropped + 2 unrelated files reformatted

**Severity:** minor
**Status:** open
**Filed:** 2026-06-26

## What's wrong

The split is functionally perfect — `cargo build`/`clippy`/`fmt`/`test` all
pass, all 800 tests green (17 dispatch tests intact), every function body /
struct / the `TOOLS` literal / every test moved byte-for-byte (`render_gemini`
diffed character-identical old-vs-new), the public surface re-exports cleanly,
and the pinned caller files (`src/ai/backends/*.rs`, `src/daemon/executor/mod.rs`)
are untouched.

But the **sorted-multiset line-fidelity acceptance criterion** (phase doc
"Acceptance criteria", last box) is **not** met. The move was required to be
verbatim with "no comment edits beyond the two new module doc-comments" (Out of
Scope, final bullet). Two distinct deviations:

### (a) Four comment lines silently dropped during the move

A sorted-multiset diff of old `src/ai/tools.rs` vs. the five new files (minus
the authorized glue: per-file `use` headers, `mod`/`pub use` blocks, the two
module doc-comments, the test `use super::super::…` import) is **not** clean.
Beyond the spec-mandated visibility changes (`fn`→`pub(super) fn`,
`struct`→`pub(super) struct`, `trait ToolArgs`→`pub(super) trait`), these
comment lines present in the old file are absent from the new files:

1. The doc comment on the private `fn dispatch` (old `src/ai/tools.rs:1614`):
   ```rust
   /// Dispatch arm helper — deserialises `args` into `T` and constructs the event.
   fn dispatch<T: ToolArgs>(id: &str, args: Value, ts: Option<String>) -> Option<AiEvent> {
   ```
   In `src/ai/tools/dispatch.rs:5` the `fn dispatch` line is present but the
   `///` doc comment above it is gone.

2. The section-header block before the `ToolArgs` trait (old
   `src/ai/tools.rs:1013–1015`):
   ```rust
   // ---------------------------------------------------------------------------
   // Tool event dispatcher (shared by all three provider backends)
   // ---------------------------------------------------------------------------
   ```
   All three lines dropped (the `// ----` separator count fell 10 → 8, and the
   `// Tool event dispatcher` line does not appear anywhere under
   `src/ai/tools/`).

Verify:
```
grep -rn 'Dispatch arm helper' src/ai/tools/      # → no match (DROPPED)
grep -rn 'Tool event dispatcher' src/ai/tools/    # → no match (DROPPED)
```

### (b) Two unrelated files reformatted into the commit

Commit `56517a7` also modified two files that are **not** part of the tools
split and **not** authorized by the phase doc ("Do not edit … any other
caller"):

- `src/cli/commands/chat.rs` (+5 −9) — `cargo fmt` line-rewrapping inside
  `banner_lines` (the `eye_markers` array, the `find().map()` chain, `sub_pad`,
  `hint_pad`).
- `src/cli/render_ratatui.rs` (+8 −1) — `cargo fmt` re-wrapping of the
  `render_spinner_region(...)` call.

Both are pure formatting (no logic change), produced as collateral when the
executor ran the STANDARDS §4-required `cargo fmt --all` (the writing form)
over a tree where these two pre-existing files were not fmt-clean. They were
neither reported in the executor's `files_changed` list nor authorized.

## What should happen

A mechanical split is verbatim: the sorted-multiset of non-blank trimmed lines
(minus the documented glue) must equal the old file's, and the commit must
touch only the files the phase authorizes. The milestone holds phases 04–06 to
exactly this bar (each "verified by a sorted-multiset line diff proving
byte-for-byte fidelity"); phase 07 must clear the same bar so the calibration
ledger records an honest result.

## How to fix

1. Restore the dropped `fn dispatch` doc comment in `src/ai/tools/dispatch.rs`,
   directly above `fn dispatch<T: ToolArgs>`:
   ```rust
   /// Dispatch arm helper — deserialises `args` into `T` and constructs the event.
   ```
2. Restore the dispatcher section-header block. The cleanest verbatim home is
   the top of the dispatch region in `src/ai/tools/dispatch.rs`, immediately
   before `fn dispatch` (after the file's `use` header):
   ```rust
   // ---------------------------------------------------------------------------
   // Tool event dispatcher (shared by all three provider backends)
   // ---------------------------------------------------------------------------
   ```
3. Revert the two unrelated formatting-only files so the commit is scoped to
   the split:
   ```
   git checkout 56517a7^ -- src/cli/commands/chat.rs src/cli/render_ratatui.rs
   ```
   These two files being fmt-dirty pre-dates this phase; bringing them into
   compliance is a separate `chore:` commit, not part of the tools split. If
   `cargo fmt --all -- --check` then fails on them, that failure is pre-existing
   and out of scope for phase 07 — leave it for a dedicated formatting chore.
4. Re-run the full command set and the sorted-multiset fidelity check; confirm
   the only remaining old-vs-new content differences are the spec-mandated
   `pub(super)` visibility changes and the authorized glue.

## Verification

- [ ] `grep -rn 'Dispatch arm helper' src/ai/tools/` matches `dispatch.rs`.
- [ ] `grep -rn 'Tool event dispatcher' src/ai/tools/` matches `dispatch.rs`.
- [ ] `git show <new-commit> --stat` lists only `src/ai/tools/*`, the deleted
      `src/ai/tools.rs`, and the doc files — no `src/cli/*` entries.
- [ ] Sorted-multiset diff (old `tools.rs` vs. new five files, minus glue)
      shows only the `pub(super)` visibility lines as differences.
- [ ] `cargo build` zero warnings; `cargo clippy --all-targets --all-features
      -- -D warnings` passes; `cargo fmt --all -- --check` passes for the
      `src/ai/tools/` files; `cargo test` passes with 773 unit + 27 integration
      (unchanged count).
