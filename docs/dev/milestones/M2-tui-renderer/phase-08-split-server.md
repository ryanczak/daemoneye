# Phase 08: Split `daemon/server.rs` into a `server/` submodule

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** done
**Depends on:** phase-07 (done)
**Estimated diff:** ~1976 lines moved (mechanical), ~50 lines new glue
**Tags:** language=rust, kind=refactor, size=l

## Goal

`src/daemon/server.rs` is 1976 lines — the second-largest source file in the
repo and the next target in the C5 oversized-file sweep. Split it into a
`src/daemon/server/` submodule of four files so each concern lives on its own:
the catch-up brief + pane-id validation (and their tests), the stateless
quick-return IPC handlers, the `handle_ask` orchestrator, and the
`handle_client` dispatch root. This is a **pure mechanical move**: no behavior
changes, no API changes, no new tests. Every existing public path
(`crate::daemon::server::*`) must resolve exactly as before.

This is the same kind of split as phase-04/05/06/07. Phase-07 (split-tools)
cleared on the **second** try — the first attempt dropped four comment lines
during a "verbatim" move and ran an over-broad `cargo fmt --all` that swept two
unrelated files into the commit. **Both of those failure modes are
pre-injected as explicit instructions below (Pre-flight 6 and step 7). Do not
repeat them.**

## Architecture references

Read before starting:

- `CLAUDE.md` § "Key files" — the table row for `src/daemon/server.rs` names its
  canonical roles: "IPC dispatch + `handle_ask` orchestrator; utility helpers
  (`build_catchup_brief`, `is_valid_pane_id`)." The split must keep all of these
  reachable at their current paths so the table stays accurate. (Do **not** edit
  CLAUDE.md in this phase — see Out of scope.)

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes
   (`git status` clean; `cargo fmt --all -- --check` clean — the tree is fmt-clean
   at the start of this phase, so any fmt dirt you see at the end is yours).
5. This is the same kind of mechanical file-split as phase-04 (`split-render`),
   phase-05 (`split-input`), phase-06 (`split-commands`), and phase-07
   (`split-tools`). Follow the same discipline: **move** code verbatim, do not
   rewrite it; preserve item order within each destination file; **preserve every
   comment, doc-comment, and `// ──` section-header bar exactly**; re-export from
   `mod.rs` so external callers are untouched.
6. **Comment fidelity is part of "verbatim."** Phase-07 bounced (bug-phase-07-1)
   because the executor silently dropped a `///` doc comment and a section-header
   comment during the move. This file has **eight `// ── … ──` section-header
   bars** and one pre-existing `// TODO(M2): consolidate params into a struct`
   line (at the head of the Ask handler). Every one of them must survive the move
   to its destination file, character-identical. The TODO is **pre-existing
   content being relocated, not a new TODO** — moving it verbatim does not violate
   STANDARDS §1's no-new-TODO rule; do **not** delete it, do **not** act on it.

## Current state

`src/daemon/server.rs` (1976 lines) is one flat file. Its top-level structure,
by line range (read the file to confirm — line numbers are a guide, not a
contract):

| Lines | Content |
|---|---|
| 1–19 | imports (19 `use` lines, listed verbatim below) |
| 32–41 | `pub(crate) fn is_valid_pane_id(id: &str) -> bool` (+ its doc comment, ~lines 21–31) |
| 43–167 | `pub(crate) fn build_catchup_brief(…) -> Option<String>` (+ its doc comment) |
| 168–367 | `pub async fn handle_client(…)` — the dispatch root (the big `match request { … }`) |
| 369–1074 | **15 quick-return handlers**, grouped under five `// ── … ──` section bars: `handle_ping`, `handle_shutdown`, `handle_refresh` (Quick-return, bar @369); `handle_set_model`, `handle_list_models` (Model management, bar @401); `handle_set_pane`, `handle_list_panes` (Pane management, bar @463); `handle_status`, `handle_query_limits`, `handle_reset_tool_count` (Status/limits, bar @573); `handle_save_session`, `handle_load_session`, `handle_list_saved_sessions`, `handle_delete_saved_session`, `handle_rename_saved_session` (Named session CRUD, bar @843) |
| 1075–1644 | `// ── Ask handler ──` bar (@1075) + `// TODO(M2): …` (@1078) + `async fn handle_ask(…)` (@1079) — includes an inner `// ── Conversation loop ──` bar (@1622) |
| 1645–1976 | `#[cfg(test)] mod tests` (~331 lines, 21 `#[test]` fns) — **all** tests cover `build_catchup_brief`, its `sum_cost_between` helper, and `is_valid_pane_id` (the catchup concerns), grouped under inner bars `// ── build_catchup_brief ──`, `// ── Phase 7: catch-up brief cost integration ──`, `// ── is_valid_pane_id ──` |

The 19 import lines (old lines 1–19), verbatim:

```rust
use crate::ai::Message;
use crate::ai::filter::mask_sensitive;
use crate::config::default_socket_path;
use crate::config::{Config, load_named_prompt};
use crate::cost::CostAttribution;
use crate::daemon::prompt::{PromptCtx, build_first_turn_prompt, build_subsequent_turn_prompt};
use crate::daemon::session::*;
use crate::daemon::stream;
use crate::daemon::utils::*;
use crate::ipc::{Request, Response};
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use libc;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::BufReader;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite};
use tokio::net::UnixStream;
```

External callers — these paths **must keep resolving unchanged**:

```
src/daemon/hook.rs:1:  use crate::daemon::server::is_valid_pane_id;
src/daemon/mod.rs:762: handle_client(stream, cache_conn, sessions_conn, sched_conn, bg_conn, managed_conn).await
```

`src/daemon/mod.rs:39` declares the module: `pub mod server;` and line 66
re-globs it: `pub use server::*;`. **Both lines stay exactly as-is** — a
directory module `src/daemon/server/mod.rs` satisfies `pub mod server;`
identically to the old `src/daemon/server.rs` file. `digest.rs:3` has a
doc-comment mention of `crate::daemon::server` (prose only, no import) — leave it.

Cross-references that cross the new file boundaries (verified by grep):

- `handle_ask` (→ `ask.rs`) calls `build_catchup_brief` (→ `catchup.rs`).
- `handle_set_pane` (one of the handlers → `handlers.rs`) calls `is_valid_pane_id`
  (→ `catchup.rs`).
- `handle_client` (→ `mod.rs`) calls **every** handler (→ `handlers.rs`) and
  `handle_ask` (→ `ask.rs`), plus the `crate::daemon::hook::*` notify handlers by
  their full paths (those paths are unchanged — leave them).
- `build_catchup_brief` references `crate::daemon::utils::sum_cost_between` by its
  full path (unchanged — leave it).

## Spec

Delete `src/daemon/server.rs` and replace it with a `src/daemon/server/`
directory of four files: `mod.rs`, `catchup.rs`, `handlers.rs`, `ask.rs`. Move
code **verbatim** — same item bodies, same comments, same `// ──` section bars,
same order within each destination.

### Import strategy (read this before writing any file)

The old file uses two glob imports (`use crate::daemon::session::*;` and
`use crate::daemon::utils::*;`) plus 17 specific imports. Rather than hand-derive
the exact per-file import set (error-prone with globs), use this deterministic
procedure for **each** new `.rs` file:

1. Start the file with the **full 19-line import header** copied verbatim from
   the list above. For the cross-file references add the matching `use super::…`
   line(s) named in each step below.
2. Build (`cargo build`) and lint (`cargo clippy --all-targets --all-features -- -D warnings`).
3. The compiler/clippy will name **each** unused import (`unused import: \`…\``,
   including any entirely-unused glob). **Remove exactly the imports it names —
   add nothing, guess nothing.** Trust the compiler over any sketch. If it names
   an *unresolved* path instead, add the `use super::…` the message points to.

The per-file sketches below are a **starting guide**, not a contract — the
compiler is the authority. (This mirrors WORKFLOW § "Verify external APIs against
live docs": pin behavior, let the tool resolve the exact structure.)

### 1. Create `src/daemon/server/catchup.rs`

Move, in this order:
- the doc comment + `pub(crate) fn is_valid_pane_id(…)` (old ~21–41),
- the doc comment + `pub(crate) fn build_catchup_brief(…)` (old 43–167),
- the **entire** `#[cfg(test)] mod tests` block (old 1645–1976), verbatim
  including its three inner `// ──` bars.

Keep `is_valid_pane_id` and `build_catchup_brief` exactly `pub(crate)` (they
already are). The test module keeps its existing header verbatim:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Message;
    …
}
```

`use super::*;` here resolves to this file's items (`build_catchup_brief`,
`is_valid_pane_id`), which is what the tests call — so it works unchanged. The
tests also reach `crate::daemon::utils::sum_cost_between`, `crate::cost::*`, etc.
by full paths already present in the test bodies; do not alter them.

Import sketch (then prune per the procedure): `use crate::ai::Message;` is the
likely survivor at file scope; the build will tell you the rest.

### 2. Create `src/daemon/server/handlers.rs`

Move the **15 quick-return handlers** (old 369–1074) here, **including all five
`// ── … ──` section bars** that group them. Preserve item order exactly.

Visibility change: each handler is currently a private `async fn`. Change all 15
from `async fn` to `pub(super) async fn` so the dispatch root in `mod.rs` (their
parent module) can call them:

- `handle_ping`, `handle_shutdown`, `handle_refresh`, `handle_set_model`,
  `handle_list_models`, `handle_set_pane`, `handle_list_panes`, `handle_status`,
  `handle_query_limits`, `handle_reset_tool_count`, `handle_save_session`,
  `handle_load_session`, `handle_list_saved_sessions`,
  `handle_delete_saved_session`, `handle_rename_saved_session`.

Do not change the generic bounds, parameters, or bodies — only the leading
`async fn` → `pub(super) async fn`.

`handle_set_pane` calls `is_valid_pane_id` (now in `catchup.rs`), so add:

```rust
use super::catchup::is_valid_pane_id;
```

Import sketch (then prune): this file uses `libc`, `ScheduleStore`,
`default_socket_path`, `Config`, the `session::*` and `utils::*` globs (for the
`send_response_split` writer + session types), `Request`/`Response`, `Arc`, and
the tokio `AsyncWrite` bound. Start with the full header + the `use super::catchup`
line and prune per the procedure.

### 3. Create `src/daemon/server/ask.rs`

Move the `// ── Ask handler ──` bar, the `// TODO(M2): consolidate params into a
struct` line, and `async fn handle_ask(…)` (old 1075–1644) here — including the
inner `// ── Conversation loop ──` bar. Preserve the TODO comment **verbatim**
(pre-existing relocation, not a new TODO — see Pre-flight 6).

Visibility change: `handle_ask` is currently a private `async fn`. Change it to
`pub(super) async fn handle_ask(…)` so `handle_client` in `mod.rs` can call it.
Do not change its parameters or body.

`handle_ask` calls `build_catchup_brief` (now in `catchup.rs`), so add:

```rust
use super::catchup::build_catchup_brief;
```

Import sketch (then prune): this is the import-heaviest file — it uses `Message`,
`mask_sensitive`, `CostAttribution`, `PromptCtx` + `build_first_turn_prompt` +
`build_subsequent_turn_prompt`, `load_named_prompt`, `Config`, `stream`,
`Instant`, the `AsyncBufRead`/`AsyncBufReadExt`/`AsyncWrite` bounds, `Arc`, the
`session::*` + `utils::*` globs, `Request`/`Response`, `ScheduleStore`,
`SessionCache`. Start with the full header + the `use super::catchup` line and
prune per the procedure.

### 4. Create `src/daemon/server/mod.rs`

The module root. It **keeps `handle_client` defined in it** (do not move
`handle_client` — it is the dispatch root and stays at the module root), declares
the three submodules, and re-exports so external paths resolve unchanged:

```rust
//! IPC server: client dispatch (`handle_client`) plus the catch-up brief,
//! quick-return handlers, and the `handle_ask` orchestrator.
//! Split across submodules in phase-08; the public surface is re-exported here.

mod ask;
mod catchup;
mod handlers;

pub(crate) use catchup::is_valid_pane_id;

use ask::handle_ask;
use handlers::*;

// … plus whatever of the 19-line header `handle_client` itself needs …
```

Then move `pub async fn handle_client(…)` (old 168–367) verbatim below the
imports, keeping it exactly `pub`.

Notes:
- `pub(crate) use catchup::is_valid_pane_id;` is **required** so
  `crate::daemon::server::is_valid_pane_id` keeps resolving for `hook.rs`. Verify
  after building: `grep -rn 'is_valid_pane_id' src/daemon/hook.rs` still compiles.
- Do **not** re-export `build_catchup_brief` from `mod.rs` — it is used only
  inside the `server` module (by `ask.rs`, via `super::catchup::build_catchup_brief`,
  and by the tests). A `pub(crate) use` re-export of it would be unused crate-wide
  and trip `clippy -D warnings`. Leave it reachable via its sibling-module path.
- `use ask::handle_ask;` and `use handlers::*;` bring the moved fns into scope so
  `handle_client`'s body (the `match request { … }` arms calling `handle_ping(…)`,
  `handle_ask(…)`, etc.) compiles **without editing the match body**.
- Import sketch for `handle_client`'s own needs (then prune): `Config`, `Request`,
  `Response`, `Arc`, `SessionCache`, `ScheduleStore`, `Result`, `BufReader`,
  `AsyncBufReadExt` (for `read_line`), `UnixStream`, the `session::*` glob
  (`SessionStore`), and the `utils::*` glob (`send_response`). Start with the full
  header and prune.

### 5. Delete the old file

Remove `src/daemon/server.rs`. `src/daemon/mod.rs` lines 39 (`pub mod server;`)
and 66 (`pub use server::*;`) are unchanged — they now resolve to the directory
module.

### 6. Format only the new files

Do **not** run `cargo fmt --all` — phase-07 bounced partly because `cargo fmt
--all` reformatted unrelated files into the commit. Format **only** the four new
files:

```sh
rustfmt src/daemon/server/mod.rs src/daemon/server/catchup.rs src/daemon/server/handlers.rs src/daemon/server/ask.rs
```

Then confirm the whole tree is still fmt-clean (it was clean at phase start, so it
must be clean now): `cargo fmt --all -- --check` → no output. If `--check` reports
a file **you did not create or move**, you have a collateral-fmt problem — revert
that file, do not commit it.

### 7. Commit

One `refactor:` commit. The diff should touch **only**: the deleted
`src/daemon/server.rs`, the four new `src/daemon/server/*.rs` files, and the two
phase-doc status updates (this file + the README row). `git diff --stat` must show
**no** changes to `src/daemon/mod.rs`, `src/daemon/hook.rs`, or any other source
file.

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all -- --check` passes (whole tree clean).
- [ ] `cargo test` passes — the **same** test count as before this phase (no tests
      added, removed, or renamed; the 21 `#[test]` fns from the old `mod tests`
      all still run and pass under `daemon::server::catchup::tests`).
- [ ] `src/daemon/server.rs` no longer exists; `src/daemon/server/` contains
      exactly `mod.rs`, `catchup.rs`, `handlers.rs`, `ask.rs`.
- [ ] `src/daemon/mod.rs` still reads `pub mod server;` (line ~39) and
      `pub use server::*;` (line ~66), both unchanged. `git diff --stat` shows
      no change to `src/daemon/mod.rs`.
- [ ] `src/daemon/hook.rs` compiles **without edits** — its
      `use crate::daemon::server::is_valid_pane_id;` line is byte-identical to
      before (`git diff --stat` shows no change to `src/daemon/hook.rs`).
- [ ] **Comment fidelity:** all eight `// ── … ──` section-header bars and the
      `// TODO(M2): consolidate params into a struct` line survive the move,
      character-identical, in their destination files. Spot-check:
      `grep -rn 'TODO(M2)' src/daemon/server/` → `ask.rs`; `grep -rcn '// ──' `
      across the four new files sums to the old file's count of those bars.
- [ ] **Line-fidelity check (sorted-multiset):** the concatenated non-blank,
      trimmed lines of the four new files, minus the new glue (the `mod` + `use`
      headers, the module doc-comment in `mod.rs`, the `use super::catchup::…`
      lines, and the `async fn` → `pub(super) async fn` visibility prefixes),
      equal the non-blank trimmed lines of the old `src/daemon/server.rs`.
      Spot-check by diffing one representative moved item (e.g. `handle_status`)
      old-vs-new — its body must be character-identical.

## Test plan

No new tests. This phase **moves** the existing `#[cfg(test)] mod tests` verbatim
into `catchup.rs`. The acceptance bar is that all 21 pre-existing tests still
compile and pass after the move. Named regression anchors that must still pass
(all now under `daemon::server::catchup::tests`):

- `catchup_brief_none_when_away_less_than_30s`,
  `catchup_brief_detects_background_task`, `catchup_brief_detects_webhook_alert`,
  `catchup_brief_counts_events_correctly` — prove `build_catchup_brief` moved intact.
- `catchup_brief_includes_cost_when_ghosts_ran`,
  `sum_cost_between_excludes_events_outside_window` — prove the cost-integration
  path and its `crate::daemon::utils::sum_cost_between` full-path reference still
  resolve across the new module boundary.
- `valid_pane_ids_accepted`, `invalid_pane_ids_rejected` — prove `is_valid_pane_id`
  moved intact and is still reachable from the in-file test module.

## End-to-end verification

Not applicable — phase ships no runtime-loadable artifact. This is a pure internal
refactor: the IPC wire protocol, every request handler's behavior, and the daemon
dispatch loop are byte-for-byte unchanged. The real-artifact guarantee is the
build + full test suite passing with an unchanged test count, plus the unchanged
`git diff --stat` on `src/daemon/mod.rs` and `src/daemon/hook.rs`.

## Authorizations

- [ ] May touch `docs/architecture.md`: **No.**
- [ ] May add dependencies: **No.**
- [ ] May edit `CLAUDE.md`: **No.** (Its file-table lists `src/daemon/server.rs`;
      updating that entry to the `server/` submodule is a follow-up doc task, not
      part of this mechanical phase — leave CLAUDE.md alone.)

Otherwise: None.

## Out of scope

- **Do not** move `handle_client` out of `mod.rs`. It is the dispatch root and
  stays at the module root; only the handlers, `handle_ask`, and the catchup
  helpers/tests are extracted.
- **Do not** act on the `// TODO(M2): consolidate params into a struct` comment —
  do not consolidate `handle_ask`'s params into a struct. Move the TODO and the
  current signature verbatim. (That consolidation, if ever done, is its own phase.)
- **Do not** rename, add, remove, merge, or re-signature any handler, helper, or
  test. The only permitted edits are the `pub(super)` visibility prefixes named in
  the Spec and the new module/import glue.
- **Do not** edit `src/daemon/mod.rs`, `src/daemon/hook.rs`, `src/daemon/digest.rs`,
  or any caller. If the re-exports in `mod.rs` are correct, no caller needs to change.
- **Do not** edit `CLAUDE.md` or `docs/architecture.md`.
- **Do not** run `cargo fmt --all` (it sweeps unrelated files into the commit —
  see step 6). Format only the four new files.
- **Do not** "improve" any code you move — no clippy-pleasing rewrites, no
  reordering of match arms, no comment edits, no dropping of section-header bars.

## Update Log

(Filled in by the executor.)

<!-- entries appended below this line -->

### Update — 2026-06-26 18:35 (started)

**Executor:** rexyMCP executor
**Action:** Begin phase-08 split of `daemon/server.rs` into `server/` submodule.

### Update — 2026-06-26 18:35 (complete)

**Summary:** Split `src/daemon/server.rs` (1976 lines) into `src/daemon/server/` submodule with four files: `mod.rs` (dispatch root + `handle_client`), `catchup.rs` (`is_valid_pane_id`, `build_catchup_brief`, 19 tests), `handlers.rs` (15 quick-return handlers), `ask.rs` (`handle_ask` orchestrator).

**Verification commands:**
- `cargo build` — zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — passed
- `cargo fmt --all -- --check` — whole tree clean
- `cargo test` — 773 passed (lib), 27 passed (bin), 0 failed; all 19 catchup tests pass under `daemon::server::catchup::tests`

**Files changed:**
- Deleted: `src/daemon/server.rs`
- Created: `src/daemon/server/mod.rs`, `src/daemon/server/catchup.rs`, `src/daemon/server/handlers.rs`, `src/daemon/server/ask.rs`
- Updated: `docs/dev/milestones/M2-tui-renderer/phase-08-split-server.md` (status: review), `docs/dev/milestones/M2-tui-renderer/README.md` (phase table row)

**Comment fidelity check:**
- `grep -rcn '// ──' src/daemon/server/` → mod.rs:0, ask.rs:2, catchup.rs:3, handlers.rs:5 = 10 total (matches original)
- `grep -rn 'TODO(M2)' src/daemon/server/` → `ask.rs:20` (survived verbatim)

**End-to-end verification:** N/A — pure internal refactor. Build + full test suite passing with unchanged test count, `git diff --stat` shows no changes to `src/daemon/mod.rs` or `src/daemon/hook.rs`.

**Notes for review:** None — mechanical split, no behavior changes.

### Review verdict — 2026-06-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (rexyMCP)
- **Scope deviations:** one **nit** — duplicated `#[cfg(test)]` attribute at
  `src/daemon/server/catchup.rs:126–127` (the original had a single
  `#[cfg(test)]` on `mod tests`). Harmless: redundant identical `cfg(test)` gates
  compile and pass `clippy -D warnings` as a no-op; no content lost, no behavior
  change. Left as-is (nit per WORKFLOW severity — executor may decline); a
  one-line cleanup if ever touched. Not bounced: disproportionate to a redundant
  no-op line on an otherwise byte-clean split.
- **Calibration:** mechanical phase, normal spec — cleared in **one dispatch**
  (129 turns), unlike phase-07 which bounced. The forward-injected bug-07 lessons
  held: comment fidelity perfect (all 10 `// ──` bars + the `// TODO(M2)` line +
  the pre-existing `#[allow(clippy::too_many_arguments)]` moved verbatim), and the
  `cargo fmt --all` collateral was avoided (commit `ef154b2` touches only the
  deleted `server.rs`, the four new files, and the two phase docs — `daemon/mod.rs`
  and `hook.rs` untouched). The `is_valid_pane_id` re-export
  (`pub(crate) use catchup::is_valid_pane_id;`) keeps `hook.rs` compiling without
  edits. Line-fidelity multiset (old vs. new, minus glue): every OLD-not-in-NEW
  line is an authorized `async fn`→`pub(super) async fn` visibility change or the
  `tokio::io` import split — **zero body content lost**. The lone artifact is the
  duplicated `#[cfg(test)]`. **Emerging trend (2 of 2 recent splits):** this
  executor introduces a small fidelity artifact on large mechanical splits (07:
  dropped 4 comments; 08: duplicated an attribute) even when bodies move
  byte-clean — worth a line in the M2 retrospective, but each artifact is
  ≤ nit/minor and caught by the multiset-diff idiom, so no fold yet.

**Independent re-run command set (separate invocations):**
- `cargo fmt --all -- --check` → clean (exit 0, whole tree)
- `cargo build` → Finished, zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` → clean
- `cargo test` → 773 unit + 27 integration pass, 2 ignored (800 total, unchanged count)

**Acceptance criteria:** all met. `src/daemon/server.rs` deleted; `src/daemon/server/`
= exactly `mod.rs`/`catchup.rs`/`handlers.rs`/`ask.rs`; `src/daemon/mod.rs` (`pub mod
server;` + `pub use server::*;`) and `src/daemon/hook.rs` untouched (`git show --stat
ef154b2` confirms neither is in the commit); 21 catchup tests run under
`daemon::server::catchup::tests`; new-file hygiene greps (unwrap/expect/panic/dbg!/
new-TODO/new-`#[allow]`) all clean — the one `#[allow(clippy::too_many_arguments)]`
and the one `// TODO(M2)` are pre-existing, moved verbatim.
