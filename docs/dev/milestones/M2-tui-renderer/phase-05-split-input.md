# Phase 05: split-input — promote `cli/input.rs` to a `cli/input/` submodule (`tty` + `editor`)

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** done
**Depends on:** phase-04 (done)
**Estimated diff:** ~374 lines moved, ~6 lines net new (two file headers + `mod`/`pub use` lines)
**Tags:** language=rust, kind=refactor, size=s

## Goal

`src/cli/input.rs` mixes two unrelated concerns: a **terminal I/O layer**
(the `/dev/tty` async wrapper, raw/cooked termios switching, and key parsing)
and a **pure line-editing layer** (`InputLine` / `InputState` — character
buffers, cursor, history navigation, no I/O at all). Promote the single file to
a `src/cli/input/` directory split into `tty.rs` (I/O) and `editor.rs` (editing),
with `input/mod.rs` re-exporting both so every existing caller keeps working
unchanged. This is a **pure mechanical move** — no behavior change — and is the
`input.rs` half of code-issue C5 (oversized `cli/` files; see milestone README).

> **Note (architect):** phase 03's legacy-path deletion already shrank
> `input.rs` from its pre-M2 size to 374 lines — under the milestone's
> `< ~800` target. So this phase is no longer about *size reduction*; it is the
> milestone-planned **separation of concerns** (I/O vs. editing) that makes the
> two halves independently testable and navigable. The split still has clear
> merit and was in the locked M2 plan; it proceeds as specced.

## Architecture references

Read before starting:

- `docs/dev/milestones/M2-tui-renderer/README.md` § "Phases" (row 05:
  "termios/`AsyncStdin` → `cli/input/tty`; `InputLine`/`InputState` editing →
  `cli/input/editor`") and the exit criterion "no behavior change in the
  extracted code." — this phase delivers that for `input.rs`.

(No `docs/architecture.md` edit is needed or authorized — this is an internal
file split, not a design change.)

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes
   (`git status` clean; `cargo build` green).

## Current state

`src/cli/input.rs` (374 lines) contains, in source order:

| Symbol | Lines | Vis | Concern → target file |
|---|---|---|---|
| `// ── Async stdin wrapper ──` banner | 1 | — | (redundant after split; see task notes) |
| `TtyFd` (struct + `Drop` + `AsRawFd`) | 3–27 | private | I/O → `tty.rs` |
| `AsyncStdin` (struct + impl: `new`, `read_byte`, `read_line`) | 29–83 | `pub` | I/O → `tty.rs` |
| `// ── Interactive line editor ──` banner | 85 | — | (redundant after split; see task notes) |
| `InputLine` (struct + impl) | 87–149 | `pub` | editing → `editor.rs` |
| `InputState` (struct + `Default` + impl) | 151–231 | `pub` | editing → `editor.rs` |
| `Key` (enum) | 233–251 | `pub` | I/O → `tty.rs` |
| `set_raw_mode` | 253–280 | `pub` | I/O → `tty.rs` |
| `restore_termios` | 282–288 | `pub` | I/O → `tty.rs` |
| `read_key` | 290–374 | `pub` | I/O → `tty.rs` |

**The two concerns are already fully decoupled (verified by grep):**

- `editor.rs`'s `InputLine` / `InputState` are pure data structures — they call
  **nothing** from the I/O half and reference only `std`. `InputState`'s private
  helpers (`InputLine::from_str`, `InputLine::as_string`) and the private fields
  (`buf`, `cursor`, `current`, `history`, `history_idx`, `saved`) are all used
  **only** within these two types, so they stay private and intra-file in
  `editor.rs`.
- `tty.rs`'s items form a closed group: `read_key` takes `&AsyncStdin` and
  returns `Key`; `AsyncStdin` wraps `TtyFd`. None of them reference `InputLine`
  or `InputState`. The I/O half references only external crates
  (`libc`, `tokio`, `anyhow`, `std`) — **all already fully-qualified at every
  use site in the current code** (e.g. `libc::open`, `tokio::io::unix::AsyncFd`,
  `std::os::unix::io::AsRawFd`, and the function-local
  `use tokio::time::{Duration, timeout};` inside `read_key`). So `tty.rs` needs
  **no** top-level `use` statements, and `editor.rs` needs **no** top-level
  `use` statements.

**No cross-file imports between `tty.rs` and `editor.rs` are required.** Neither
half uses the other.

**`unsafe` in the I/O half is pre-existing and moves verbatim.** `TtyFd::drop`,
`AsyncStdin::new`, `set_raw_mode`, and `restore_termios` contain `unsafe` blocks
(libc FFI). STANDARDS §1 forbids *new* `unsafe` and tells you to file a blocker
if you think you need one — that rule does **not** apply here: you are
relocating existing `unsafe` byte-for-byte, not writing new `unsafe`. Do **not**
refactor it, do **not** wrap or "fix" it, and do **not** file a blocker about
it. Move it exactly as-is.

**External callers (must keep compiling with zero changes):**

```
src/cli/commands/mod.rs:3      use crate::cli::input::*;
src/cli/commands/mod.rs:97     crate::cli::input::AsyncStdin::new()?
src/cli/render_ratatui.rs:1    use crate::cli::input::InputLine;
src/cli/commands/ask.rs:10     use crate::cli::input::AsyncStdin;
src/cli/commands/stream.rs:10  use crate::cli::input::*;
src/cli/commands/stream.rs:570 use crate::cli::input::InputLine;
src/cli/commands/stream.rs:728 crate::cli::input::InputLine::new()
```

Every one of these resolves a symbol at the path `crate::cli::input::<Name>`.
The re-export in task 3 preserves that path exactly, so **none of these call
sites change.**

The cli module tree is declared in `src/cli/mod.rs`:

```rust
pub mod commands;
pub(crate) mod diff;
pub mod input;
pub mod local_cmds;
pub mod markdown;
pub mod notify;
pub mod render;
pub mod render_ratatui;
pub mod status;
```

`pub mod input;` already names the module. After this phase `input` resolves to
`input/mod.rs` instead of `input.rs` — **`src/cli/mod.rs` does not change.**

## Spec

Numbered tasks in execution order. This is a move-and-re-export refactor:
preserve each moved item's body **byte-for-byte**. No visibility changes are
needed for any symbol. Build after the structural steps so a missing item
surfaces immediately.

1. **Create `src/cli/input/editor.rs`** — move `InputLine` (struct + impl,
   input.rs 87–149) and `InputState` (struct + `impl Default` + impl, 151–231)
   into this new file, in that order. Keep `InputLine` and `InputState` `pub`
   (unchanged) and their helpers/fields private (unchanged). This file needs no
   `use` statements. You may keep or drop the `// ── Interactive line editor ──`
   banner (line 85) at the top — it is now a redundant file-level header; either
   choice is fine and has zero behavior impact.

2. **Create `src/cli/input/tty.rs`** — move, in this order: `TtyFd` (struct +
   `Drop` + `AsRawFd`, input.rs 3–27), `AsyncStdin` (struct + impl, 29–83),
   `Key` (enum, 233–251), `set_raw_mode` (253–280), `restore_termios` (282–288),
   and `read_key` (290–374). Keep `TtyFd` private and
   `AsyncStdin`/`Key`/`set_raw_mode`/`restore_termios`/`read_key` `pub`
   (all unchanged). This file needs no top-level `use` statements (the
   `use tokio::time::{Duration, timeout};` inside `read_key` stays where it is,
   function-local). The `unsafe` blocks move verbatim (see Current state). You
   may keep or drop the `// ── Async stdin wrapper ──` banner (line 1).

3. **Create `src/cli/input/mod.rs`** — the new module root. Its entire contents:

   ```rust
   mod editor;
   mod tty;

   pub use editor::*;
   pub use tty::*;
   ```

   The two submodules are private (`mod`, not `pub mod`); the glob re-exports
   surface every `pub` item at `crate::cli::input::<Name>`, which is exactly the
   path the external callers (Current state) already use. This keeps the public
   API byte-identical and requires **zero** caller changes.

4. **Delete the old `src/cli/input.rs`** — once `input/mod.rs`, `input/tty.rs`,
   and `input/editor.rs` exist and contain all the moved code, remove the
   original flat file. (`git mv` into the directory + edits, or copy+delete, are
   both fine — end state is the directory, no leftover `input.rs`.)

5. **Build and verify zero behavior change** — run the full command set
   (Acceptance criteria below). All existing callers must compile unchanged and
   the full test suite must stay green, proving the public API and behavior are
   identical after the move.

## Acceptance criteria

Verifiable conditions:

- [ ] `src/cli/input/mod.rs`, `src/cli/input/tty.rs`, and `src/cli/input/editor.rs`
      exist, and `src/cli/input.rs` no longer exists (`test ! -f src/cli/input.rs`).
- [ ] `editor.rs` contains `InputLine` and `InputState` and **not** `AsyncStdin`,
      `TtyFd`, `Key`, `set_raw_mode`, `restore_termios`, or `read_key`. Verify:
      `grep -nE 'AsyncStdin|struct TtyFd|enum Key|fn set_raw_mode|fn restore_termios|fn read_key' src/cli/input/editor.rs`
      prints nothing.
- [ ] `tty.rs` contains the I/O group and **not** the editor types. Verify:
      `grep -nE 'struct InputLine|struct InputState' src/cli/input/tty.rs`
      prints nothing.
- [ ] `src/cli/input/mod.rs` is under 15 lines and contains both
      `pub use editor::*;` and `pub use tty::*;`.
- [ ] `src/cli/mod.rs` is unchanged (still `pub mod input;`, no new lines).
- [ ] No caller file (`commands/mod.rs`, `commands/ask.rs`, `commands/stream.rs`,
      `render_ratatui.rs`) needed an edit to the `crate::cli::input::…` paths —
      they compile as-is.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes (in
      particular: no unused-import warning on the surviving
      `use crate::cli::input::*;` globs in callers, and no dead-code or
      unreachable-`pub` warning on the moved items or the re-export).
- [ ] `cargo test` passes with **no net change in test count** (input.rs has no
      co-located tests; none are added or removed).

## Test plan

No new tests. `input.rs` has **no** co-located `#[cfg(test)]` module, and
`src/cli/tests.rs` does not reference any input symbol (verified by grep), so
there is nothing to relocate. The behavior-preservation guard is the existing
compile + clippy + full test suite: the external callers exercise `AsyncStdin`,
`InputLine`, `InputState`, `Key`, and `read_key`, so a broken extraction
(misplaced item, dropped `pub`, missing re-export) fails the build rather than
passing silently.

Adding tests is **out of scope** for this phase (it is a mechanical move).

## End-to-end verification

Not applicable — phase ships no runtime-loadable real artifact. It is a pure
internal module split with no change to any binary entrypoint, CLI behavior, or
on-disk config. Quote the acceptance-criteria grep + `test -f` + `cargo test`
tail outputs in the completion Update Log in lieu of an E2E transcript.

## Authorizations

- [x] May create new files: `src/cli/input/mod.rs`, `src/cli/input/tty.rs`,
      `src/cli/input/editor.rs`.
- [x] May delete `src/cli/input.rs` (it is replaced by the directory of the
      same module name).

No edit to `src/cli/mod.rs` is needed (or authorized) — `pub mod input;` already
resolves to the new directory. No new dependencies. No `docs/architecture.md`
edit. No `unsafe` is *added* (existing `unsafe` is moved verbatim, per Spec).

## Out of scope

- Splitting `src/cli/commands/mod.rs` (phase 06). Do not touch it beyond
  confirming it still compiles — it should need **no** edits.
- Any behavior change to terminal I/O, key parsing, raw/cooked-mode switching,
  line editing, or history navigation. No refactoring of function bodies "while
  you're in there" — move them verbatim.
- Changing the visibility of any moved item (none needs it), or "tidying" the
  `unsafe` blocks.
- Adding tests, doc-comment rewrites, or renaming any symbol.
- Repointing caller imports — the re-export in `input/mod.rs` makes every
  existing `crate::cli::input::…` path resolve unchanged; do **not** edit call
  sites.
- Further sub-splitting `tty.rs` (e.g. separating key-parsing from the termios
  wrapper) — the two-file split (`tty` + `editor`) is the intended granularity.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-26 14:03 (started)

**Executor:** rexyMCP executor

Split `src/cli/input.rs` into `src/cli/input/editor.rs` (InputLine, InputState) and `src/cli/input/tty.rs` (TtyFd, AsyncStdin, Key, set_raw_mode, restore_termios, read_key), with `src/cli/input/mod.rs` re-exporting both.

### Update — 2026-06-26 14:03 (complete)

**Executor:** rexyMCP executor

**Summary:** Pure mechanical split of `src/cli/input.rs` into `src/cli/input/` submodule with `editor.rs` (pure line-editing types) and `tty.rs` (terminal I/O layer). Zero behavior change.

**Files changed:**
- Created: `src/cli/input/editor.rs` (145 lines — InputLine, InputState)
- Created: `src/cli/input/tty.rs` (224 lines — TtyFd, AsyncStdin, Key, set_raw_mode, restore_termios, read_key)
- Created: `src/cli/input/mod.rs` (5 lines — mod declarations + glob re-exports)
- Deleted: `src/cli/input.rs` (374 lines)
- Updated: `docs/dev/milestones/M2-tui-renderer/phase-05-split-input.md` (status: todo → review)
- Updated: `docs/dev/milestones/M2-tui-renderer/README.md` (phase table: todo → review)

**Verification commands (all passed):**
- `cargo fmt --all` — clean
- `cargo build` — clean, zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test` — 27 passed, 0 failed, 2 ignored (no net change)

**Acceptance-criteria grep results:**
- `test -f src/cli/input/mod.rs` ✓, `test -f src/cli/input/tty.rs` ✓, `test -f src/cli/input/editor.rs` ✓, `test ! -f src/cli/input.rs` ✓
- `grep -nE 'AsyncStdin|struct TtyFd|enum Key|fn set_raw_mode|fn restore_termios|fn read_key' src/cli/input/editor.rs` — no output (clean)
- `grep -nE 'struct InputLine|struct InputState' src/cli/input/tty.rs` — no output (clean)
- `mod.rs` is 5 lines, contains both `pub use editor::*;` and `pub use tty::*;`
- `src/cli/mod.rs` unchanged (still `pub mod input;`)
- No caller files needed edits

**End-to-end verification:** N/A — pure internal module split with no runtime-loadable artifact.

**Notes for review:** None. This is a byte-for-byte move with glob re-exports preserving all existing `crate::cli::input::…` paths.

### Review verdict — 2026-06-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (rexyMCP executor)
- **Scope deviations:** none — the only departure from a literal byte-copy is
  dropping the two redundant section-banner comments (`// ── Async stdin wrapper ──`,
  `// ── Interactive line editor ──`) and their 3 surrounding blank lines, which the
  Spec (tasks 1–2) explicitly authorized ("You may keep or drop the banner").
- **Calibration:** lean spec cleared first try (this is a normally-specced mechanical
  split, not a rewrite phase — low calibration signal, as planned in the milestone's
  calibration protocol). Independent re-run of the full command set: `cargo fmt`,
  `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings` (forced
  rebuild of the input module — zero warnings), `cargo test` (773 + 27 passed, 2
  ignored, 0 failed — no net change).

**Deep-review axes (M2 directive):**

1. **Spec conformance** — all 5 tasks implemented; every acceptance criterion verified:
   three new files exist + `input.rs` gone (`test ! -f` ✓); `editor.rs` holds only
   `InputLine`/`InputState`, `tty.rs` holds only the I/O group (both negative greps
   clean); `mod.rs` is 5 lines with both glob re-exports; `src/cli/mod.rs` unchanged;
   no caller file (`commands/mod.rs`, `ask.rs`, `stream.rs`, `render_ratatui.rs`) was
   edited (`git diff --stat` empty). Stayed inside boundaries — phase 06's
   `commands/mod.rs` untouched; no new deps; `unsafe` moved verbatim, not refactored.
2. **Reasoning quality** — correctly identified the two concerns as fully decoupled and
   produced a clean two-file split with no cross-imports and no top-level `use`
   statements (the function-local `use tokio::time::{Duration, timeout};` in `read_key`
   stayed function-local, as specced). Faithful mechanical move — exactly the task.
3. **Code & test quality** — byte-for-byte move proven by sorted multiset line diff of
   the original `input.rs` (374 lines) vs the concatenated new files (369 lines): the
   only delta is the 2 authorized banner comments + 3 blank lines; every line of code
   is preserved. No new `unwrap`/`expect`/`panic!`, no `TODO`/`dbg!`/`#[allow]`, no
   commented-out code in the new files. No new tests (correctly — `input.rs` had no
   co-located tests; behavior preservation is guarded by the compile + full suite the
   external callers exercise). One conventional commit (`refactor(cli): split input.rs
   into input/ submodule (tty + editor)`).
