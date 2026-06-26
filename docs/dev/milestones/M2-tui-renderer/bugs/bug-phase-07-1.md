# Bug 1 on phase-07: verbatim-move fidelity broken — 4 comment lines dropped + 2 unrelated files reformatted

**Severity:** minor
**Status:** fixed
**Filed:** 2026-06-26
**Resolved:** 2026-06-26 — part (a) fixed (dropped comments restored); part (b)
superseded (see resolution note at end).

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

- [x] `grep -rn 'Dispatch arm helper' src/ai/tools/` matches `dispatch.rs:9`.
- [x] `grep -rn 'Tool event dispatcher' src/ai/tools/` matches `dispatch.rs:6`.
- [x] Fix commit `0a2b258` `--stat` lists only `src/ai/tools/dispatch.rs` and the
      two doc files — no `src/cli/*` entries.
- [x] Sorted-multiset content now clean: `render_gemini` re-diffed
      character-identical old→new; `// -----` separator count back to 10 (matches
      the pre-split original); only the spec-mandated `pub(super)` visibility lines
      remain as content differences.
- [x] `cargo build` zero warnings; `cargo clippy --all-targets --all-features
      -- -D warnings` passes; `cargo fmt --all -- --check` passes (whole tree);
      `cargo test` passes with 773 unit + 27 integration (unchanged count).

## Resolution (2026-06-26)

**Part (a) — dropped comments: fixed.** Both the `/// Dispatch arm helper` doc
comment and the 3-line `// Tool event dispatcher` section header were restored in
`src/ai/tools/dispatch.rs` (commit `0a2b258`); separator-line fidelity is back to
10 ≡ 10.

**Part (b) — revert the two collateral `cargo fmt` files: superseded, not
enforced.** The executor did not perform the revert (the files remain in their
`56517a7`-reformatted state). On review this was **accepted rather than
re-bounced**, because the instruction conflicts with phase-07 acceptance criterion
#3 (`cargo fmt --all -- --check` passes): reverting makes the tree fmt-dirty and
breaks that DoD box, which is currently green tree-wide. This is the second M2
occurrence of the "post-write formatting collateral" class, for which WORKFLOW.md
already prescribes architect-resolve-at-close-out and warns the spec/bounce route
is ineffective — confirmed here by the executor failing the revert twice. The
bundled fmt-clean state is the desired end state; the residue is commit-scope
hygiene only (the unrelated fmt belongs in a separate `chore:`), recorded as
`scope_deviation` in the phase-07 review verdict.
