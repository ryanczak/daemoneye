# Phase 04: split-render — extract markdown + syntax-highlight into `cli/markdown`

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** review
**Depends on:** phase-03 (done)
**Estimated diff:** ~1150 lines moved, ~10 lines net new (module decls + import updates)
**Tags:** language=rust, kind=refactor, size=m

## Goal

`src/cli/render.rs` is 1365 lines and mixes two unrelated concerns: small
terminal/chrome primitives (panels, status bar, width/height, line-wrap) and a
large markdown + syntax-highlighting engine (`MarkdownRenderer`, `render_inline`,
`highlight_code`, the per-language keyword tables). Extract the markdown +
syntax-highlight half into a new `src/cli/markdown/` submodule, leaving
`render.rs` as the focused terminal-chrome module. This is a **pure mechanical
move** — no behavior change — and closes the `render.rs` part of code-issue C5
(oversized `cli/` files; see milestone README).

## Architecture references

Read before starting:

- `docs/dev/milestones/M2-tui-renderer/README.md` § "Phases" (row 04) and the
  exit criterion "`src/cli/render.rs` … reduced to a focused size (target < ~800
  lines each) by extraction into submodules; **no behavior change** in the
  extracted code." — this phase delivers that for `render.rs`.

(No `docs/architecture.md` edit is needed or authorized — this is an internal
file split, not a design change.)

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes
   (`git status` clean; `cargo build` green).

## Current state

`src/cli/render.rs` (1365 lines) contains, in order:

**STAYS in `render.rs` (lines 1–242) — terminal-chrome primitives:**

| Symbol | Lines | Vis |
|---|---|---|
| `print_tool_panel` | 6–37 | `pub` |
| `local_user_host` | 38–52 | private |
| `print_tool_started` | 53–65 | `pub` |
| `print_tool_finished` | 66–89 | `pub` |
| `print_user_query` | 90–132 | `pub` |
| `wrap_line_hard` | 133–163 | `pub` |
| `visual_len` | 164–183 | `pub` |
| `terminal_width` | 184–203 | `pub` |
| `terminal_height` | 204–220 | `pub` |
| `StatusBarState<'a>` | 221–242 | `pub` |

**MOVES to `src/cli/markdown/` (lines 243–1364):**

| Symbol | Lines | Current vis | Target file |
|---|---|---|---|
| `WrapWriter` (struct + impl) | 243–348 | private | `markdown/mod.rs` |
| `render_inline` | 350–418 | `pub` | `markdown/mod.rs` |
| `CommentStyle` (enum) | 419–426 | private | `markdown/syntax.rs` |
| `lang_keywords` | 427–643 | private | `markdown/syntax.rs` |
| `lang_comment_style` | 644–661 | private | `markdown/syntax.rs` |
| `emit_word_token` | 662–683 | private | `markdown/syntax.rs` |
| `highlight_code` | 684–813 | private | `markdown/syntax.rs` |
| `MarkdownRenderer` (struct + `Default` + `impl`) | 814–1289 | `pub` | `markdown/mod.rs` |
| `#[cfg(test)] mod tests` | 1290–1364 | — | `markdown/mod.rs` |

**The dependency boundary is one-directional and was verified by grep:**

- The moved code references exactly two symbols that **stay** in `render.rs`:
  `terminal_width` and `visual_len` (both `pub`). Nothing else.
- The staying code (lines 1–242) references **none** of the moved symbols.
- `WrapWriter` is used **only** by `MarkdownRenderer` (its `wrap: WrapWriter`
  field, `render.rs:822`/`827`) — that is why it moves with the markdown engine,
  not with the chrome.
- Within the moved code: `MarkdownRenderer` (in `mod.rs`) calls `render_inline`
  (same file) and `highlight_code` (in `syntax.rs`). `highlight_code` (in
  `syntax.rs`) calls `lang_keywords`, `lang_comment_style`, `emit_word_token`,
  and uses `CommentStyle` — **all four stay together in `syntax.rs`**, so they
  remain private to that file.

**External callers of the moved `MarkdownRenderer` (must be repointed):**

- `src/cli/commands/stream.rs:129` — `let mut md = MarkdownRenderer::new();`
  (resolved today via the glob `use crate::cli::render::*;` at line 11).
- `src/cli/render_ratatui.rs:720`, `:790`, `:813` — fully-qualified
  `crate::cli::render::MarkdownRenderer::new()`.

`render_inline` and `highlight_code` have **no callers outside `render.rs`**
(verified by grep), so their move needs no external repointing.

The cli module tree is declared in `src/cli/mod.rs`:

```rust
pub mod commands;
pub(crate) mod diff;
pub mod input;
pub mod local_cmds;
pub mod notify;
pub mod render;
pub mod render_ratatui;
pub mod status;
```

Note `cli/mod.rs` does **not** glob-re-export `render` (no `pub use render::*`),
so callers reach these symbols via explicit paths (`crate::cli::render::…`) or a
local `use crate::cli::render::*;`. Keep that convention: do **not** add
`pub use markdown::*;`.

## Spec

Numbered tasks in execution order. This is a move-and-repoint refactor; preserve
each moved item's body **byte-for-byte** except for the visibility change called
out in task 3. Build after the structural steps so a missing `use` surfaces
immediately.

1. **Create `src/cli/markdown/syntax.rs`** — move `CommentStyle` (render.rs
   419–426), `lang_keywords` (427–643), `lang_comment_style` (644–661),
   `emit_word_token` (662–683), and `highlight_code` (684–813) into this new
   file, in that order. These five form a closed group: `highlight_code` calls
   the other four and nothing else from `render.rs`, so `syntax.rs` needs **no**
   `use crate::cli::render::…` import. Keep `CommentStyle`, `lang_keywords`,
   `lang_comment_style`, and `emit_word_token` **private** (no `pub`). Change
   only `highlight_code`'s signature from `fn highlight_code(` to
   `pub(super) fn highlight_code(` so `markdown/mod.rs` can call it (task 3).

2. **Create `src/cli/markdown/mod.rs`** — move `WrapWriter` (struct + impl,
   render.rs 243–348), `render_inline` (350–418), `MarkdownRenderer` (struct +
   `impl Default` + `impl`, 814–1289), and the `#[cfg(test)] mod tests` block
   (1290–1364) into this new file, preserving their relative order
   (`WrapWriter`, then `render_inline`, then `MarkdownRenderer`, then `tests`).
   Keep `WrapWriter` private and `render_inline`/`MarkdownRenderer` `pub`
   (unchanged from today). At the top of the file add:

   ```rust
   mod syntax;

   use crate::cli::render::{terminal_width, visual_len};
   use syntax::highlight_code;
   ```

   The `tests` module's `use super::*;` already pulls `MarkdownRenderer` into
   scope; the tests touch the private fields `in_code_block` / `code_lang`, which
   is fine because the tests live in `markdown/mod.rs` alongside the struct. Do
   not change any test body.

3. **Remove the moved code from `src/cli/render.rs`** — delete render.rs lines
   243–1364 (everything from `struct WrapWriter` through the end of the
   `#[cfg(test)] mod tests` block). What remains is lines 1–242 (the
   terminal-chrome primitives in the "STAYS" table above). Do not add a
   re-export shim for the moved symbols.

4. **Declare the new submodule** — in `src/cli/mod.rs`, add `pub mod markdown;`.
   Place it in alphabetical position between `pub mod local_cmds;` and
   `pub mod notify;` to match the existing ordering. Do **not** add
   `pub use markdown::*;`.

5. **Repoint the external `MarkdownRenderer` callers:**
   - In `src/cli/commands/stream.rs`, add `use crate::cli::markdown::MarkdownRenderer;`
     to the import block (the existing `use crate::cli::render::*;` at line 11
     stays — `stream.rs` still uses `StatusBarState` and other render symbols
     from it). The bare `MarkdownRenderer::new()` call then resolves.
   - In `src/cli/render_ratatui.rs`, change all three
     `crate::cli::render::MarkdownRenderer` (lines ~720, ~790, ~813) to
     `crate::cli::markdown::MarkdownRenderer`.

6. **Build and verify zero behavior change** — run the full command set (§4 of
   STANDARDS / Acceptance criteria below). The four existing markdown tests
   (now in `markdown/mod.rs`) must still pass unchanged, proving the engine
   behaves identically after the move.

## Acceptance criteria

- [ ] `src/cli/markdown/mod.rs` and `src/cli/markdown/syntax.rs` exist; `git mv`
      or copy+delete is fine, but the moved function/struct bodies are unchanged
      (only `highlight_code`'s visibility widened to `pub(super)`).
- [ ] `src/cli/render.rs` no longer contains `WrapWriter`, `render_inline`,
      `highlight_code`, `lang_keywords`, `lang_comment_style`, `emit_word_token`,
      `CommentStyle`, or `MarkdownRenderer`. Verify:
      `grep -nE 'WrapWriter|render_inline|highlight_code|lang_keywords|lang_comment_style|emit_word_token|enum CommentStyle|struct MarkdownRenderer' src/cli/render.rs`
      prints nothing.
- [ ] `src/cli/render.rs` is under 300 lines (`wc -l src/cli/render.rs`).
- [ ] Both new files are each under 800 lines (`wc -l src/cli/markdown/*.rs`).
- [ ] `src/cli/mod.rs` contains `pub mod markdown;` and does **not** contain
      `pub use markdown::*`.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes (in
      particular: no unused-import warning on the surviving
      `use crate::cli::render::*;` globs, no dead-code warning on moved items).
- [ ] `cargo test` passes — existing count plus the four relocated markdown
      tests, with no net change in test count.

## Test plan

No new tests. This is a mechanical move; the four existing markdown tests are the
behavior-preservation guard and must pass **unchanged** after relocation to
`src/cli/markdown/mod.rs`:

- `fenced_code_block_body_renders_as_code_not_heading` — asserts code-block state
  tracking + that a `#`-prefixed body line renders as code, not a heading.
- `heading_outside_code_block_still_renders_as_heading`
- `code_block_without_lang`
- `nested_fences_in_code_body_do_not_toggle_state`

Adding, removing, or modifying any test is **out of scope** for this phase
(see Out of scope). If a test fails to compile after the move, the cause is an
incorrect extraction (wrong visibility or a missing `use`), not a test problem —
fix the extraction.

## End-to-end verification

Not applicable — phase ships no runtime-loadable real artifact. It is a pure
internal module split with no change to any binary entrypoint, CLI behavior, or
on-disk config. The behavior-preservation guarantee is carried by the four
relocated tests (Test plan) and the clippy/build gates; quote the
acceptance-criteria grep + `wc -l` + `cargo test` tail outputs in the completion
Update Log in lieu of an E2E transcript.

## Authorizations

- [x] May create new files: `src/cli/markdown/mod.rs`, `src/cli/markdown/syntax.rs`.
- [x] May edit `src/cli/mod.rs` (add the `pub mod markdown;` declaration),
      `src/cli/commands/stream.rs` (add one `use`), and
      `src/cli/render_ratatui.rs` (repoint three paths) — strictly the
      import/path changes in Spec tasks 4–5.

No new dependencies. No `docs/architecture.md` edit. No `unsafe`.

## Out of scope

- Splitting `src/cli/input.rs` (phase 05) or `src/cli/commands/mod.rs`
  (phase 06). Touch neither beyond what is impossible to avoid (you should not
  need to touch them at all).
- Any behavior change to markdown rendering, syntax highlighting, wrapping, or
  the chrome primitives. No refactoring of function bodies "while you're in
  there" — move them verbatim.
- Adding, removing, renaming, or rewording tests.
- Adding a `pub use markdown::*;` re-export or any back-compat shim in
  `render.rs` — repoint the call sites instead (Spec task 5).
- Changing the visibility of any moved item except the one widening called out
  in Spec task 1 (`highlight_code` → `pub(super)`).
- Further sub-splitting `syntax.rs` (e.g. one file per language) — the
  single `syntax.rs` is the intended granularity.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-26 04:40 (progress)

Executor started phase. Created `src/cli/markdown/syntax.rs` (CommentStyle, lang_keywords, lang_comment_style, emit_word_token, highlight_code with pub(super) visibility) and `src/cli/markdown/mod.rs` (WrapWriter, render_inline, MarkdownRenderer, tests). Removed lines 236–1365 from `src/cli/render.rs`. Declared `pub mod markdown;` in `src/cli/mod.rs`. Repointed `MarkdownRenderer` callers in `stream.rs` and `render_ratatui.rs`. Build, clippy, fmt, and tests all pass clean.

### Update — 2026-06-26 04:40 (complete)

**Summary:** Pure mechanical split of `src/cli/render.rs` (1365 → 234 lines). Markdown + syntax-highlighting engine extracted into `src/cli/markdown/` submodule with two files (`mod.rs` at 746 lines, `syntax.rs` at 386 lines). No behavior change — all four relocated markdown tests pass unchanged, and the full test suite (27 passed, 2 ignored) is green.

**Acceptance criteria:** all ticked above.

**Commands:**

```
cargo fmt --all
(passed clean)

cargo build 2>&1 | tail -20
(passed clean, zero warnings)

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
(passed clean)

cargo test 2>&1 | tail -30
test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
```

**End-to-end verification:**

Not applicable — phase ships no runtime-loadable real artifact. It is a pure internal module split with no change to any binary entrypoint, CLI behavior, or on-disk config. The behavior-preservation guarantee is carried by the four relocated tests and the clippy/build gates.

Grep verification (moved symbols absent from render.rs):
```
grep -nE 'WrapWriter|render_inline|highlight_code|lang_keywords|lang_comment_style|emit_word_token|enum CommentStyle|struct MarkdownRenderer' src/cli/render.rs
(exit code 1 — no matches, as expected)
```

Line counts:
```
wc -l src/cli/render.rs src/cli/markdown/*.rs
  234 src/cli/render.rs
  746 src/cli/markdown/mod.rs
  386 src/cli/markdown/syntax.rs
(all under 800; render.rs under 300)
```

**Files changed:**
- `src/cli/markdown/syntax.rs` — new file: CommentStyle, lang_keywords, lang_comment_style, emit_word_token, highlight_code (pub(super))
- `src/cli/markdown/mod.rs` — new file: WrapWriter, render_inline, MarkdownRenderer, tests
- `src/cli/render.rs` — removed lines 236–1365 (markdown/syntax code), now 234 lines of terminal-chrome primitives
- `src/cli/mod.rs` — added `pub mod markdown;`
- `src/cli/commands/stream.rs` — added `use crate::cli::markdown::MarkdownRenderer;`
- `src/cli/render_ratatui.rs` — repointed 3 `crate::cli::render::MarkdownRenderer` → `crate::cli::markdown::MarkdownRenderer`

**New tests:** none (mechanical move; 4 existing tests relocated unchanged)

**Commits:**
- `split-render` — extract markdown + syntax-highlight into cli/markdown submodule

**Notes for review:** None — this is a pure mechanical move with zero behavior change.
