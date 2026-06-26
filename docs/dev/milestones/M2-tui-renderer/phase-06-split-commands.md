# Phase 06: split-commands — extract the interactive chat loop from `cli/commands/mod.rs` into a `chat` submodule

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** done
**Depends on:** phase-05 (done)
**Estimated diff:** ~638 lines moved, ~12 lines net new (one import block + `mod chat;` + `use chat::run_chat_inner;` + one `pub(super)`)
**Tags:** language=rust, kind=refactor, size=s

## Goal

`src/cli/commands/mod.rs` (679 lines) is both the `commands` **module root**
(declaring the sibling submodules and re-exporting the public command entry
points) **and** the entire implementation of the interactive `daemoneye chat`
loop (auto-attach, terminal/pane setup, the ratatui chat loop, slash-command
handling, and the input-reading key loop). Extract the chat implementation into
a new `src/cli/commands/chat.rs` submodule, leaving `mod.rs` as a thin module
root plus the public `run_chat` entry wrapper. This is the `commands/mod.rs`
half of code-issue C5 (oversized `cli/` files; see milestone README) and the
final M2 split phase.

This is a **pure mechanical move** — no behavior change. Every moved item is
relocated verbatim; the only edits are the `use` paths (sibling submodules go
from bare names to `super::`-qualified) and a single visibility widening
(`run_chat_inner` becomes `pub(super)` so the wrapper in `mod.rs` can call it).

> **Note (architect):** like phase 05, this phase is no longer about hitting the
> milestone's `< ~800` size target — phase 03's legacy-path deletion already
> brought `mod.rs` to 679 lines. It is the milestone-planned **separation of
> concerns**: the `commands` module root (wiring + public command surface) vs.
> the interactive-chat implementation. The split was in the locked M2 plan and
> proceeds as specced.

## Architecture references

Read before starting:

- `docs/dev/milestones/M2-tui-renderer/README.md` § "Phases" (row 06:
  "split-commands — extract `run_chat_inner_raw` loop + ctx structs + slash help
  from `cli/commands/mod.rs`") and the exit criterion "no behavior change in the
  extracted code." — this phase delivers that for `commands/mod.rs`.

(No `docs/architecture.md` edit is needed or authorized — this is an internal
file split, not a design change.)

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes
   (`git status` clean; `cargo build` green).

## Current state

`src/cli/commands/mod.rs` (679 lines) contains, in source order:

| Lines | Item | Disposition |
|---|---|---|
| 1–6 | top-level `use` block (`anyhow::Result`, `crate::cli::input::*`, `crate::cli::render::*`, `RatatuiRendererStdout`, `Config`) | **rewrite** (see Spec task 1) |
| 8–22 | `mod` declarations + `pub use` re-exports of the command entry points | **stays** in `mod.rs` (add one `mod chat;`) |
| 24–27 | private `use` of `SessionApproval`, `new_session_id`, `resolve_target_pane`, and the `stream::{…}` group | **moves** to `chat.rs` (re-pathed; see Spec) |
| 29–40 | `pub async fn run_chat` (error-wrapping public entry) | **stays** in `mod.rs` |
| 42–179 | `async fn run_chat_inner` (auto-attach + session/pane/width setup; ends by calling `run_chat_inner_raw`) | **moves** to `chat.rs`, becomes `pub(super)` |
| 181–207 | structs `InputHandles`, `TerminalCtx`, `TmuxCtx`, `RatatuiCtx` | **moves** to `chat.rs` (stay private) |
| 209–242 | `async fn run_chat_inner_raw` (destructures handles, calls `run_chat_ratatui`) | **moves** to `chat.rs` (stays private) |
| 243–570 | `async fn run_chat_ratatui` (greeting, `help_text`, the slash-command loop, send-query) | **moves** to `chat.rs` (stays private) |
| 572–588 | struct `RatatuiInputCtx` | **moves** to `chat.rs` (stays private) |
| 590–679 | `async fn read_input_line_inner_ratatui` (the key-reading `select!` loop) | **moves** to `chat.rs` (stays private) |

**Key facts established by grep (do not re-derive — they shape the split):**

- **Only `run_chat` is referenced outside this file.** `src/main.rs:323` calls
  `cli::run_chat(session).await?` (re-exported via `pub use commands::*;` in
  `src/cli/mod.rs:13`). **Nothing** outside `mod.rs` references `run_chat_inner`,
  `run_chat_inner_raw`, `run_chat_ratatui`, `read_input_line_inner_ratatui`, or
  any of the five ctx structs (`InputHandles`, `TerminalCtx`, `TmuxCtx`,
  `RatatuiCtx`, `RatatuiInputCtx`). So all of those can move into `chat.rs` and
  **stay private** — no `pub` is needed on any of them.
- **The wrapper/implementation boundary is `run_chat` ↔ `run_chat_inner`.**
  `run_chat` (stays) calls `run_chat_inner` (moves). That is the *only*
  cross-module call after the split, so `run_chat_inner` is the *only* item
  whose visibility must widen — to `pub(super)` (visible to the parent
  `commands` module, i.e. `mod.rs`). Everything else moved is constructed **and**
  consumed entirely within `chat.rs`, so it stays private.
- **`mod.rs` has no `#[cfg(test)]` module** (verified by grep) — there are no
  co-located tests to relocate.
- **The four ctx structs that cross between `run_chat_inner` and
  `run_chat_inner_raw` (`InputHandles`/`TerminalCtx`/`TmuxCtx`) move *together*
  with both functions**, so their construction site and consumption site both
  land in `chat.rs` — no field-visibility widening is needed on any of them.
  This is why moving `run_chat_inner` (not just `run_chat_inner_raw`) is the
  clean cut: it keeps every struct private.

**Symbols the moved code uses (these drive `chat.rs`'s import block):**

- `crate::cli::input::*` — `InputState`, `AsyncStdin`, `InputLine`, `Key`,
  `read_key`.
- `crate::cli::render::*` — `StatusBarState`, `terminal_width`.
- `crate::cli::render_ratatui::RatatuiRendererStdout`.
- `crate::config::Config` (`Config::load`).
- `super::approval::SessionApproval`, `super::ipc_client::new_session_id`,
  `super::pane::resolve_target_pane` — these three are **sibling** submodules of
  `chat` under `commands`, so from inside `chat.rs` they are reached via
  `super::`.
- `super::stream::{AskTmuxCtx, QueryArgs, RatatuiQueryCtx, TokenCtx,
  ask_with_session_ratatui}` — same sibling rule. (Compare `src/cli/commands/
  ask.rs:17`, which already imports this exact group as
  `use super::stream::{…};` — copy that idiom.)
- `uuid::Uuid` is used **fully-qualified** at the old line 450
  (`uuid::Uuid::new_v4()`); it needs **no** `use` statement and must not get one.
- `crate::tmux::*`, `std::*`, `tokio::*` are all used fully-qualified at their
  sites (e.g. `crate::tmux::session_exists`, `tokio::time::timeout`,
  `tokio::signal::unix::{SignalKind, signal}` as a function-local `use`), so
  they need **no** top-level `use` in `chat.rs`.

**What `mod.rs`'s surviving `run_chat` needs:** only `anyhow::Result` (its return
type) and `run_chat_inner` (which it calls). Its body uses `std::io` and
`eprintln!`/`eprint!` fully-qualified / via macro. So after the move `mod.rs`
must drop the now-unused top-level imports (`crate::cli::input::*`,
`crate::cli::render::*`, `RatatuiRendererStdout`, `Config`) and the private
`use` block (24–27) — leaving them in `mod.rs` would trip
`clippy -D warnings` (unused import). This is the single highest-risk part of
the phase; the exact target import blocks are given verbatim below.

## Spec

Numbered tasks in execution order. This is a move-and-re-path refactor: preserve
every moved item's body **byte-for-byte** except the two edits explicitly called
out (the sibling `use` paths gain `super::`, and `run_chat_inner` gains
`pub(super)`). `cargo fmt --all` will normalize `use`-statement ordering — do not
hand-fight it; the blocks below show the *set* of imports, not a fmt-final order.

1. **Rewrite `src/cli/commands/mod.rs` to the module-root + wrapper only.** After
   this task `mod.rs`'s entire contents are:

   ```rust
   use anyhow::Result;

   mod approval;
   mod ask;
   mod chat;
   mod costs;
   mod ipc_client;
   mod lifecycle;
   mod pane;
   mod setup;
   mod stream;

   pub use ask::run_ask;
   pub use costs::{GroupBy, run_costs};
   pub use ipc_client::{connect, recv, send_request};
   pub use lifecycle::{run_logs, run_ping, run_stop};
   pub use setup::run_setup;

   use chat::run_chat_inner;

   pub async fn run_chat(session_override: Option<String>) -> Result<()> {
       let result = run_chat_inner(session_override).await;
       if let Err(ref e) = result {
           // AsyncStdin has been dropped by now; synchronous stdin is safe.
           use std::io::Write;
           eprintln!("\n\x1b[31m✗\x1b[0m daemoneye error: {}", e);
           eprint!("\x1b[2mPress Enter to close this pane…\x1b[0m");
           std::io::stderr().flush().ok();
           let _ = std::io::stdin().read_line(&mut String::new());
       }
       result
   }
   ```

   The `run_chat` body is **identical** to the current lines 29–40 — do not
   change it. The only additions vs. today are the `mod chat;` declaration
   (alphabetically among the other `mod` lines) and `use chat::run_chat_inner;`.
   The removed lines are the four no-longer-used top-level imports (old 3–6) and
   the private `use` block (old 24–27), which all move to `chat.rs`.

2. **Create `src/cli/commands/chat.rs`** and move the chat implementation into
   it — old `mod.rs` lines **42 through 679** (`run_chat_inner` through the end
   of `read_input_line_inner_ratatui`), in the same order, **verbatim**, with
   exactly two edits:

   - Change the signature of `run_chat_inner` from
     `async fn run_chat_inner(` to `pub(super) async fn run_chat_inner(`.
     (This is the **only** visibility change in the phase.)
   - The sibling-module `use` paths are re-pathed with `super::` (they were bare
     in `mod.rs` because the submodules were declared there; from inside
     `chat.rs` the siblings are one level up).

   The file's top-level `use` block is exactly this set:

   ```rust
   use anyhow::Result;

   use crate::cli::input::*;
   use crate::cli::render::*;
   use crate::cli::render_ratatui::RatatuiRendererStdout;
   use crate::config::Config;

   use super::approval::SessionApproval;
   use super::ipc_client::new_session_id;
   use super::pane::resolve_target_pane;
   use super::stream::{AskTmuxCtx, QueryArgs, RatatuiQueryCtx, TokenCtx, ask_with_session_ratatui};
   ```

   Everything below the import block — `run_chat_inner` (now `pub(super)`), the
   structs `InputHandles` / `TerminalCtx` / `TmuxCtx` / `RatatuiCtx`,
   `run_chat_inner_raw`, the `// ── Ratatui chat loop ──` banner + `run_chat_ratatui`,
   `RatatuiInputCtx`, and `read_input_line_inner_ratatui` — is moved verbatim. Do
   **not** add `pub`/`pub(super)` to any of these (they stay private); do **not**
   reorder, rename, merge, or "tidy" any of them; do **not** add a `uuid` import
   (the call stays fully-qualified).

3. **Build and verify zero behavior change** — run the full command set
   (Acceptance criteria below). `mod.rs`'s `run_chat` must still compile and call
   into `chat::run_chat_inner`; `src/main.rs`'s `cli::run_chat` call site must
   compile unchanged; the full test suite must stay green, proving the public API
   and behavior are identical after the move.

## Acceptance criteria

Verifiable conditions:

- [ ] `src/cli/commands/chat.rs` exists and `src/cli/commands/mod.rs` still
      exists (it is **not** deleted — it remains the module root).
- [ ] `src/cli/commands/mod.rs` contains `pub async fn run_chat` and **not**
      `run_chat_inner_raw`, `run_chat_ratatui`, `read_input_line_inner_ratatui`,
      or any ctx-struct definition. Verify:
      `grep -nE 'fn run_chat_inner_raw|fn run_chat_ratatui|fn read_input_line_inner_ratatui|struct InputHandles|struct TerminalCtx|struct TmuxCtx|struct RatatuiCtx|struct RatatuiInputCtx' src/cli/commands/mod.rs`
      prints nothing.
- [ ] `src/cli/commands/mod.rs` contains `mod chat;` and `use chat::run_chat_inner;`.
- [ ] `src/cli/commands/chat.rs` contains `pub(super) async fn run_chat_inner` and
      defines all five ctx structs and the three moved private functions. Verify
      both:
      `grep -nE 'pub\(super\) async fn run_chat_inner' src/cli/commands/chat.rs`
      prints a match, and
      `grep -nE 'fn run_chat_inner_raw|fn run_chat_ratatui|fn read_input_line_inner_ratatui|struct RatatuiCtx|struct RatatuiInputCtx' src/cli/commands/chat.rs`
      prints five matches.
- [ ] `chat.rs` does **not** define `pub async fn run_chat` (the public wrapper
      stays in `mod.rs`). Verify:
      `grep -nE 'fn run_chat\b' src/cli/commands/chat.rs` prints nothing.
- [ ] `chat.rs` does **not** import `uuid` (the call is fully-qualified). Verify:
      `grep -nE '^use uuid|use uuid::' src/cli/commands/chat.rs` prints nothing.
- [ ] `src/cli/mod.rs` is unchanged (still `pub mod commands;` and
      `pub use commands::*;`).
- [ ] `src/main.rs` is unchanged (the `cli::run_chat(session)` call site needs no
      edit).
- [ ] `cargo fmt --all` passes.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes — in
      particular **no unused-import warning** in `mod.rs` (the dropped top-level
      and private imports), **no unused-import warning** in `chat.rs`, and no
      dead-code or unreachable-`pub` warning on `run_chat_inner` or the moved
      private items.
- [ ] `cargo test` passes with **no net change in test count** (no tests are
      added, removed, or relocated).

## Test plan

No new tests. `commands/mod.rs` has **no** co-located `#[cfg(test)]` module
(verified by grep), so there is nothing to relocate. The behavior-preservation
guard is the existing compile + clippy + full test suite: `src/main.rs` exercises
`run_chat`, which now routes through `chat::run_chat_inner`, so a broken
extraction (a misplaced item, a dropped/added visibility modifier, a wrong `use`
path, a leftover unused import) fails the build or clippy rather than passing
silently.

Adding tests is **out of scope** for this phase (it is a mechanical move).

## End-to-end verification

Not applicable — phase ships no runtime-loadable real artifact. It is a pure
internal module split: the `daemoneye chat` entrypoint already exists, its
behavior is unchanged, and the interactive chat loop cannot be exercised
hermetically (it needs a live tmux client, a running daemon, and an API key).
The compile + `clippy -D warnings` + full test suite is the behavior-preservation
guard. Quote the acceptance-criteria grep + `test -f` + `cargo build`/`cargo test`
tail outputs in the completion Update Log in lieu of an E2E transcript.

## Authorizations

- [x] May create a new file: `src/cli/commands/chat.rs`.
- [x] May widen the visibility of **`run_chat_inner` only**, from private to
      `pub(super)`, so the wrapper in `mod.rs` can call it. No other symbol's
      visibility changes.

No edit to `src/cli/mod.rs` or `src/main.rs` is needed (or authorized) — the
re-export chain (`pub use commands::*;`) and the `cli::run_chat` call site resolve
unchanged because `run_chat` stays in `mod.rs`. No new dependencies. No
`docs/architecture.md` edit. No new `unsafe`.

## Out of scope

- Splitting any other `cli/` file (phases 04 and 05 already handled
  `render.rs` and `input.rs`). Touch only `commands/mod.rs` and the new
  `commands/chat.rs`.
- Any behavior change to the chat loop, auto-attach, pane/width setup,
  slash-command handling, the greeting, the status bar, or key handling. No
  refactoring of function bodies "while you're in there" — move them verbatim.
- Changing the visibility of any moved item other than `run_chat_inner` (e.g.
  do **not** make the ctx structs or the other private functions `pub`/`pub(super)`
  — they are constructed and consumed entirely within `chat.rs`).
- Further sub-splitting `chat.rs` (e.g. separating the input key-loop or the
  slash-command dispatch into their own files) — the single-file `chat` submodule
  is the intended granularity for this phase.
- Repointing the `src/main.rs` call site or the `src/cli/mod.rs` re-export — they
  resolve unchanged.
- Adding tests, doc-comment rewrites, or renaming any symbol.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-26 14:38 (started)

**Executor:** rexyMCP executor

Extracting the interactive chat loop from `cli/commands/mod.rs` into a new `cli/commands/chat.rs` submodule. Pure mechanical move — no behavior change.

### Update — 2026-06-26 14:38 (complete)

**Summary:** Extracted 638 lines of chat-loop implementation from `src/cli/commands/mod.rs` into new `src/cli/commands/chat.rs`. `mod.rs` reduced from 679 to 32 lines (module root + `run_chat` wrapper only). `run_chat_inner` widened to `pub(super)`. All sibling-module imports re-pathed with `super::`.

**Acceptance-criteria verification:**

- `test -f src/cli/commands/chat.rs && test -f src/cli/commands/mod.rs` → PASS (both exist)
- `grep -nE 'fn run_chat_inner_raw|fn run_chat_ratatui|fn read_input_line_inner_ratatui|struct InputHandles|struct TerminalCtx|struct TmuxCtx|struct RatatuiCtx|struct RatatuiInputCtx' src/cli/commands/mod.rs` → no output (PASS — no moved items remain in mod.rs)
- `grep -nE 'mod chat;|use chat::run_chat_inner;' src/cli/commands/mod.rs` → `5:mod chat;` and `19:use chat::run_chat_inner;` (PASS)
- `grep -nE 'pub\(super\) async fn run_chat_inner' src/cli/commands/chat.rs` → `13:pub(super) async fn run_chat_inner` (PASS)
- `grep -nE 'fn run_chat_inner_raw|fn run_chat_ratatui|fn read_input_line_inner_ratatui|struct RatatuiCtx|struct RatatuiInputCtx' src/cli/commands/chat.rs` → 5 matches (PASS)
- `grep -nE 'fn run_chat\b' src/cli/commands/chat.rs` → no output (PASS — public wrapper stays in mod.rs)
- `grep -nE '^use uuid|use uuid::' src/cli/commands/chat.rs` → no output (PASS — uuid fully-qualified)
- `src/cli/mod.rs` unchanged, `src/main.rs` unchanged (PASS — no edits needed)
- `cargo fmt --all` → PASS (clean)
- `cargo build` → PASS (zero warnings)
- `cargo clippy --all-targets --all-features -- -D warnings` → PASS (no unused-import, no dead-code)
- `cargo test` → PASS (27 passed, 0 failed, 2 ignored — no net change in test count)

**Verification summary:** cargo fmt clean, cargo build zero warnings, clippy -D warnings clean, cargo test 27 passed 0 failed.

**Files changed:**
- `src/cli/commands/mod.rs` — rewritten to module root + `run_chat` wrapper (32 lines, was 679)
- `src/cli/commands/chat.rs` — created (649 lines, moved from mod.rs)

**End-to-end verification:** N/A — pure internal module split, no runtime-loadable artifact. Compile + clippy + test suite serve as behavior-preservation guard.

**Notes for review:** None — this is a verbatim mechanical move with exactly two edits (sibling `use` paths gain `super::`, `run_chat_inner` gains `pub(super)`).

### Review verdict — 2026-06-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (rexyMCP executor)
- **Scope deviations:** none
- **Calibration:** lean spec not in play here (this is a normally-specced mechanical
  split, per the milestone calibration protocol — splits 04–06 yield little
  density-probe signal). Cleared first try.

**Independent command re-run (reviewer host):**

- `cargo fmt --all -- --check` → exit 0 (clean)
- `cargo build` → exit 0, zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` → exit 0
- `cargo test` → 773 unit + 27 integration passed, 0 failed, 2 ignored — no net
  change from the 800-test baseline (no tests added/removed/relocated, as specced)

**Move-fidelity proof (sorted-multiset line diff, old `mod.rs` vs new `mod.rs` +
`chat.rs`):** old 679 lines → new 32 + 650 = 682 (+3 net). Every line delta is an
authorized edit and nothing else:

1. `async fn run_chat_inner` → `pub(super) async fn run_chat_inner` (the one
   authorized visibility widening).
2. `+ mod chat;` and `+ use chat::run_chat_inner;` in `mod.rs`.
3. `+ use anyhow::Result;` second copy (chat.rs needs its own).
4. Four sibling imports re-pathed bare → `super::` (`approval`, `ipc_client`,
   `pane`, `stream`).

The +3 net = `mod chat;` + `use chat::run_chat_inner;` + duplicated `use
anyhow::Result;`. **No function body line changed, nothing dropped, nothing
reordered** — a faithful byte-for-byte move.

**Three-axis assessment (M2 directive):**

1. **Spec conformance** — all 14 acceptance criteria verified by the prescribed
   greps (both-files-exist, no moved items in `mod.rs`, `mod chat;` +
   `use chat::run_chat_inner;` present, `pub(super)` on `run_chat_inner`, 5 moved
   items in `chat.rs`, no `run_chat`/`uuid` in `chat.rs`, `cli/mod.rs` and
   `main.rs` untouched). Stayed strictly inside boundaries: only `commands/mod.rs`
   and new `commands/chat.rs` touched; no other `cli/` file split; no behavior
   change. chat.rs import block matches the spec's prescribed set verbatim.
2. **Reasoning quality** — picked the clean cut correctly: moving `run_chat_inner`
   (not just `run_chat_inner_raw`) keeps all five ctx structs private (construction
   and consumption both land in `chat.rs`), so only one visibility widened. The
   `uuid::Uuid::new_v4()` call was correctly left fully-qualified (no spurious
   import). Dropped the now-unused top-level imports from `mod.rs` cleanly (clippy
   -D warnings confirms no unused-import leak — the single highest-risk part of the
   phase, handled correctly).
3. **Code & test quality** — no error-suppressing idioms, `unsafe`, `#[allow]`, or
   `#[ignore]` introduced (verbatim move; greps confirm none in `chat.rs`). No new
   tests, as specced (no co-located `#[cfg(test)]` existed to relocate). Idiomatic
   module-root/submodule layout.

**Minor (nit, not bounced):** the commit `d4e4600` swept in a pre-existing
`Cargo.toml`/`Cargo.lock` version bump (0.9.7 → 0.9.9) that was already in the dirty
working tree before this phase ran. It is **not** a dependency change and not an
executor-authored logical change — just untracked pre-existing state captured by
`git add`. Noted for commit hygiene; does not affect correctness or the DoD.
