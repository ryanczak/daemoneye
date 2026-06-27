# Phase 15: split-knowledge

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** todo
**Depends on:** none (independent C5 cleanup; touches only `src/daemon/executor/knowledge.rs` and the new `knowledge/` directory)
**Estimated diff:** ~1341 lines moved (mechanical; net behavior change = 0)
**Tags:** language=rust, kind=refactor, size=l

> **Spec density: NORMAL (mechanical split).** This is a verbatim move-and-re-path split with
> no design discovery — the same shape as phases 04–06 / 08 / 09 / 12 / 13 / 14, all of which
> cleared first try. The layout, the symbol placement, the visibility changes, the re-pathing,
> and the re-exports are **fully pinned below**. Do not redesign, rename, reorder, or "improve"
> any code: move it verbatim and fix only the module paths and the visibility qualifiers named
> below. A byte-for-byte fidelity check (sorted-multiset line diff) is the acceptance gate.
>
> **This file is shaped like phase 12 (`file_ops`), not phase 14 (`background`).** Three facts
> make it so, all verified against the current source — rely on them, but let the compiler
> confirm:
> 1. **`super::` → `super::super::` re-pathing IS required.** Unlike `background.rs` (which
>    reached every parent symbol via `crate::`-absolute paths), `knowledge.rs` reaches its
>    parent module `executor` via **relative `super::`** in six places: `use super::ToolCallOutcome;`,
>    `use super::USER_PROMPT_TIMEOUT;` (lines 1–2), and `super::foreground::is_shell_prompt`
>    (4 call sites in `watch_pane`). Once the code moves one level deeper (into `knowledge/<sub>.rs`),
>    `super` no longer means `executor` — it means `knowledge`. Every one of these must become
>    `super::super::…` (matching the phase-12 convention: `file_ops/read.rs:1` is literally
>    `use super::super::ToolCallOutcome;`). This is the exact gotcha phase 12 had.
> 2. **Re-export visibility bumps ARE required (the phase-12 E0364 pattern).** Today the 23
>    consumer-facing functions are `pub(super)` (visible to `executor`). After the split they
>    live one level deeper in leaf submodules, and `knowledge/mod.rs` re-exports them up to
>    `executor` with `pub(super) use`. **A `pub(super) use` of a `pub(super)` item fails to
>    compile with E0364** (the re-export's visibility exceeds the item's). The fix, exactly as in
>    phase 12 (`file_ops/read.rs:80` is `pub async fn run_read_file`, not `pub(super)`): bump each
>    re-exported leaf function from `pub(super)` to **`pub`**. The complete closed list is in
>    §Visibility.
> 3. **There is NO test module.** `knowledge.rs` contains zero `#[cfg(test)]` blocks (verified:
>    `grep -c '#\[cfg(test)\]'` = 0). Nothing to relocate or split on the test side — every test
>    that exercises this code lives elsewhere and reaches it through the unchanged `knowledge::`
>    paths.

## Goal

Split the oversized `src/daemon/executor/knowledge.rs` (1341 lines) into a `knowledge/` submodule
directory — `artifacts`, `memory`, `pane`, `ghost`, `agents`, plus the shared `ArtifactCtx` /
`track_artifact` and the re-exports in `mod.rs` — to close part of code-issue C5 (oversized files).
This is the **final** file in the M2 C5 split sweep. **No behavior change.** Every function, struct,
const, and impl moves verbatim to its new home; only module paths and the named item visibilities change.

## Architecture references

Read before starting:

- `docs/ROADMAP.md` §2.2 **C5** (oversized files) — why this split exists.
- The prior C5 split this one mirrors most closely: `src/daemon/executor/file_ops/` (phase 12).
  It is the precise template — same parent module (`executor`), same `super::super::` re-pathing
  of `ToolCallOutcome`, same `pub(super)`→`pub` leaf bump + `pub(super) use` re-export to dodge
  E0364, and the same idiom of keeping **shared module-level helpers in `mod.rs`** (file_ops/mod.rs
  keeps `sq_escape`/`to_hex`/`resolve_path_for_guard` as private fns the submodules reach via
  `super::…`). Read `src/daemon/executor/file_ops/mod.rs` and `src/daemon/executor/file_ops/read.rs`
  (first 5 lines) before starting — they show every pattern this phase uses.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc.
3. Read `src/daemon/executor/file_ops/mod.rs` and the first 5 lines of
   `src/daemon/executor/file_ops/read.rs` — the verbatim pattern to mirror.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Capture a fidelity baseline before touching anything (used in Acceptance):

   ```sh
   grep -vE '^\s*(//|$)' src/daemon/executor/knowledge.rs | sed 's/^[[:space:]]*//' | sort > /tmp/kn_before.txt
   wc -l /tmp/kn_before.txt   # expect 1217
   ```

## Current state

`src/daemon/executor/knowledge.rs` is one flat 1341-line file. It is a **child of `executor`**,
declared in `src/daemon/executor/mod.rs` line 3 as `mod knowledge;` (private). Its top-level
imports (lines 1–14):

```rust
use super::ToolCallOutcome;
use super::USER_PROMPT_TIMEOUT;
use crate::ai::filter::mask_sensitive;
use crate::daemon::session::{
    FG_HOOK_COUNTER, SessionStore, append_session_message, bg_done_subscribe,
};
use crate::daemon::utils::send_response_split;
use crate::daemon::utils::{log_event, normalize_output};
use crate::ipc::{Request, Response, RunbookListItem, ScriptListItem};
use crate::scheduler::ScheduleStore;
use crate::scripts;
use crate::util::UnpoisonExt;
use std::sync::Arc;
use std::time::Duration;
```

Its items, in source order, with the submodule each moves to:

| Item | Lines | Kind / current visibility | Belongs in |
|---|---|---|---|
| `ArtifactCtx<'a>` (+ 6 `pub` fields) | 20–27 | `struct` (`pub(super)`) | **`mod.rs`** |
| `track_artifact` | 29–46 | `fn` (private) | **`mod.rs`** |
| `write_script` | 52–119 | `pub(super) async fn` | `artifacts.rs` |
| `list_scripts` | 121–139 | `pub(super) async fn` | `artifacts.rs` |
| `read_script` | 141–146 | `pub(super) fn` | `artifacts.rs` |
| `delete_script` | 148–210 | `pub(super) async fn` | `artifacts.rs` |
| `write_runbook` | 216–288 | `pub(super) async fn` | `artifacts.rs` |
| `delete_runbook` | 290–361 | `pub(super) async fn` | `artifacts.rs` |
| `read_runbook` | 363–368 | `pub(super) fn` | `artifacts.rs` |
| `list_runbooks` | 370–395 | `pub(super) async fn` | `artifacts.rs` |
| `add_memory` | 401–432 | `pub(super) fn` | `memory.rs` |
| `update_memory` | 434–495 | `pub(super) fn` (`#[allow(clippy::too_many_arguments)]`) | `memory.rs` |
| `delete_memory` | 497–520 | `pub(super) fn` | `memory.rs` |
| `read_memory` | 522–539 | `pub(super) fn` | `memory.rs` |
| `list_memories` | 541–599 | `pub(super) async fn` | `memory.rs` |
| `search_repository` | 605–608 | `pub(super) fn` | `memory.rs` |
| `close_bg_window` | 614–671 | `pub(super) fn` | `pane.rs` |
| `list_panes` | 677–771 | `pub(super) fn` | `pane.rs` |
| `WatchHookGuard` (+ `Drop` impl) | 780–791 | `struct` (private) | `pane.rs` |
| `watch_pane` | 793–973 | `pub(super) fn` | `pane.rs` |
| `spawn_ghost` | 979–1068 | `pub(super) async fn` | `ghost.rs` |
| `create_agent` | 1076–1173 | `pub(super) async fn` (`#[allow(clippy::too_many_arguments)]`) | `agents.rs` |
| `read_agent` | 1175–1210 | `pub(super) fn` | `agents.rs` |
| `list_agents_tool` | 1212–1233 | `pub(super) async fn` | `agents.rs` |
| `delete_agent` | 1235–1294 | `pub(super) async fn` | `agents.rs` |
| `await_agent_result` | 1300–1341 | `pub(super) async fn` | `agents.rs` |

Note `update_memory` and `create_agent` each carry a `// TODO(M2): consolidate params into a struct`
line immediately above their `#[allow(clippy::too_many_arguments)]`. **Move both lines verbatim**
with the function — the TODO is pre-existing and **not** in scope to act on (it would otherwise
trip STANDARDS §1's no-TODO rule; it is explicitly grandfathered here as a verbatim move).

**External consumers** — verified, the **only** file outside `knowledge.rs` that names any of these
items is `src/daemon/executor/mod.rs`. It calls all 23 `pub(super)` functions and constructs
`knowledge::ArtifactCtx` (line 209) + reads `artifact_ctx.namespaces` (line 507), always via the
`knowledge::<Name>` path. (`src/daemon/executor/foreground.rs:87` mentions `knowledge::watch_pane`
only in a code comment — not a reference.)

So **every `pub(super)` function must remain reachable as `knowledge::<Name>` after the split**, and
`knowledge::ArtifactCtx` must remain reachable with its `pub` fields. The re-exports in the new
`mod.rs` (§Spec item 1) preserve this. **`src/daemon/executor/mod.rs` must not be edited** — the
re-exports keep every call site byte-for-byte valid.

**Shared items** (`ArtifactCtx`, `track_artifact`) are used across multiple submodules:

- `ArtifactCtx` — `write_script`/`write_runbook` (artifacts), `add_memory` (memory), `create_agent`
  (agents) all take `&ArtifactCtx<'_>`. It is also constructed and field-read by the external
  `executor::mod`. → lives in `mod.rs`.
- `track_artifact` — called by `write_script`/`write_runbook` (artifacts), `add_memory` (memory),
  `create_agent` (agents). Private; only called from within `knowledge`. → lives in `mod.rs`.
  A child module may call an ancestor's **private** item via `super::`, so `track_artifact` stays
  private (no bump) and the submodules import it with `use super::{ArtifactCtx, track_artifact};`.

**`is_shell_prompt` cross-reference** (`watch_pane`, now in `pane.rs`): reached today as
`super::foreground::is_shell_prompt` (4 sites). After the move it must become
`super::super::foreground::is_shell_prompt`. `is_shell_prompt` is `pub(super)` in
`executor/foreground.rs:91` (visible to `executor`); the deeper path still resolves to the same
item with the same visibility, so **`foreground.rs` needs no edit**.

## Spec

Create `src/daemon/executor/knowledge/` and delete the flat `knowledge.rs`. The `mod knowledge;`
declaration in `executor/mod.rs` line 3 is **unchanged** — Rust resolves `knowledge` to either
`knowledge.rs` or `knowledge/mod.rs`.

Land each sub-deliverable and `cargo build`-green before the next. Suggested order: `mod.rs`
skeleton → `artifacts` → `memory` → `pane` → `ghost` → `agents`, building after each.

1. **`knowledge/mod.rs` — submodule declarations + shared items + re-exports.** It holds the two
   shared items (`ArtifactCtx`, `track_artifact`) verbatim from old lines 20–46, **plus** the
   declarations and re-exports. Create it as:

   ```rust
   mod agents;
   mod artifacts;
   mod ghost;
   mod memory;
   mod pane;

   pub(super) use agents::{
       await_agent_result, create_agent, delete_agent, list_agents_tool, read_agent,
   };
   pub(super) use artifacts::{
       delete_runbook, delete_script, list_runbooks, list_scripts, read_runbook, read_script,
       write_runbook, write_script,
   };
   pub(super) use ghost::spawn_ghost;
   pub(super) use memory::{
       add_memory, delete_memory, list_memories, read_memory, search_repository, update_memory,
   };
   pub(super) use pane::{close_bg_window, list_panes, watch_pane};

   use crate::daemon::session::SessionStore;

   // ── ArtifactCtx + track_artifact moved verbatim from old lines 20–46 ──
   pub(super) struct ArtifactCtx<'a> { /* …six pub fields, unchanged… */ }

   fn track_artifact(ctx: &ArtifactCtx<'_>, kind: &str, name: &str) { /* …unchanged… */ }
   ```

   The submodules are **private** (`mod`, not `pub mod`) — consumers reach functions through the
   re-exported `knowledge::<Name>` paths. `ArtifactCtx` stays **`pub(super)`** (it is directly in
   `knowledge`, so `pub(super)` = visible to `executor`, exactly as today) and its six fields stay
   **`pub`** (read by `executor::mod`). `track_artifact` stays **private**. `mod.rs` needs only the
   one `use crate::daemon::session::SessionStore;` import (for `ArtifactCtx`'s `sessions` field and
   `track_artifact`'s body; `ArtifactRef` is reached fully-qualified as
   `crate::session_store::ArtifactRef`, unchanged).

2. **`knowledge/artifacts.rs`** — move verbatim: `write_script`, `list_scripts`, `read_script`,
   `delete_script`, `write_runbook`, `delete_runbook`, `read_runbook`, `list_runbooks` (old lines
   52–395, including the `// Scripts` / `// Runbooks` section-comment banners). Apply the 8
   `pub(super)`→`pub` bumps (§Visibility). Add the imports per §Re-pathing, including
   `use super::{ArtifactCtx, track_artifact};` and `use super::super::{ToolCallOutcome, USER_PROMPT_TIMEOUT};`.

3. **`knowledge/memory.rs`** — move verbatim: `add_memory`, `update_memory` (with its TODO +
   `#[allow]` lines), `delete_memory`, `read_memory`, `list_memories`, `search_repository` (old lines
   401–608, including the `// Memory` / `// Search / context` banners). Apply the 6 `pub(super)`→`pub`
   bumps. Add imports per §Re-pathing, including `use super::{ArtifactCtx, track_artifact};` and
   `use super::super::ToolCallOutcome;`.

4. **`knowledge/pane.rs`** — move verbatim: `close_bg_window`, `list_panes`, `WatchHookGuard` (+ its
   `Drop` impl and the doc-comment above it), `watch_pane` (old lines 614–973, including the
   `// Background window management` / `// List panes` / `// Watch pane` banners). Apply the 3
   `pub(super)`→`pub` bumps. **Re-path the 4 `super::foreground::is_shell_prompt` → `super::super::foreground::is_shell_prompt`.**
   Add imports per §Re-pathing. **`pane.rs` does NOT import `ToolCallOutcome`** — none of its three
   functions return it (all return `String`).

5. **`knowledge/ghost.rs`** — move verbatim: `spawn_ghost` (old lines 979–1068, including the
   `// Spawn ghost shell` banner **and** the two function-body `use` statements at old lines 987–988,
   which move inside the function unchanged). Apply the 1 `pub(super)`→`pub` bump. Add imports per
   §Re-pathing (`use super::super::ToolCallOutcome;` + `use crate::daemon::session::SessionStore;`).

6. **`knowledge/agents.rs`** — move verbatim: `create_agent` (with its TODO + `#[allow]` lines),
   `read_agent`, `list_agents_tool`, `delete_agent`, `await_agent_result` (old lines 1076–1341,
   including the `// Agents` / `// Await agent result` banners). Apply the 5 `pub(super)`→`pub` bumps.
   Add imports per §Re-pathing, including `use super::{ArtifactCtx, track_artifact};` and
   `use super::super::{ToolCallOutcome, USER_PROMPT_TIMEOUT};`.

7. **Delete** the old `src/daemon/executor/knowledge.rs`.

### Visibility — the only non-mechanical edits allowed

Two classes of change, nothing else:

**(a) Re-exported leaf functions: `pub(super)` → `pub`.** Because they now live one level deeper and
are re-exported up to `executor` via `pub(super) use` in `mod.rs`, leaving them `pub(super)` produces
**E0364** (re-export visibility exceeds item visibility — the exact phase-12 failure). Bump all 23:

- **artifacts.rs (8):** `write_script`, `list_scripts`, `read_script`, `delete_script`,
  `write_runbook`, `delete_runbook`, `read_runbook`, `list_runbooks`.
- **memory.rs (6):** `add_memory`, `update_memory`, `delete_memory`, `read_memory`, `list_memories`,
  `search_repository`.
- **pane.rs (3):** `close_bg_window`, `list_panes`, `watch_pane`.
- **ghost.rs (1):** `spawn_ghost`.
- **agents.rs (5):** `create_agent`, `read_agent`, `list_agents_tool`, `delete_agent`,
  `await_agent_result`.

**(b) Nothing else changes visibility.** In particular:

- `ArtifactCtx` stays **`pub(super)`** and its six fields stay **`pub`** (it lives in `mod.rs`, so
  `pub(super)` already = visible to `executor`; no bump needed and none allowed).
- `track_artifact` stays **private** (called only by descendant submodules via `super::`, which is
  legal for an ancestor-private item — no bump).
- `WatchHookGuard` and its `Drop` impl stay **private** (internal to `pane.rs`).

Adding `pub`/`pub(super)`/`pub(crate)` anywhere not listed in (a) is an unrequested change and will
bounce.

### Re-pathing — `super::` → `super::super::`; everything else `crate::`-absolute (unchanged)

Three re-pathing changes, all forced by the code moving one level deeper:

- `use super::ToolCallOutcome;` → `use super::super::ToolCallOutcome;` (in every submodule that uses
  it — see table; **not** `pane.rs`).
- `use super::USER_PROMPT_TIMEOUT;` → `use super::super::USER_PROMPT_TIMEOUT;` (artifacts, agents).
- The 4 `super::foreground::is_shell_prompt` call sites in `watch_pane` →
  `super::super::foreground::is_shell_prompt` (these are inline call expressions, not `use` lines).

All `crate::`-absolute paths (`crate::ai::…`, `crate::daemon::…`, `crate::ipc::…`, `crate::scripts`,
`crate::scheduler::…`, `crate::util::…`, `crate::memory::…`, `crate::runbook::…`, `crate::agents::…`,
`crate::header::…`, `crate::manifest::…`, `crate::search::…`, `crate::config::…`, `crate::webhook::…`,
`crate::tmux::…`, `crate::session_store::…`) are **depth-independent — copy them unchanged.**

The new **cross-module imports** (reaching the shared items in `mod.rs`):
`use super::{ArtifactCtx, track_artifact};` in `artifacts.rs`, `memory.rs`, and `agents.rs`
(`pane.rs` and `ghost.rs` use neither shared item).

**Top-level import partitioning.** Each submodule begins with the subset of the original lines 1–14
imports its moved code references. The partition is verified below; still, after writing each file,
rely on `cargo build` (missing import) + `cargo clippy -D warnings` (unused import) to converge — do
not carry an import a file does not use, and do not omit one it does. A grouped `use` (e.g.
`use crate::ipc::{Request, Response, RunbookListItem, ScriptListItem};`) splitting into a per-file
subset is a rendering change of the same import lines (note it in the fidelity diff).

| Import | mod | artifacts | memory | pane | ghost | agents |
|---|:--:|:--:|:--:|:--:|:--:|:--:|
| `super::super::ToolCallOutcome` | | ✓ | ✓ | | ✓ | ✓ |
| `super::super::USER_PROMPT_TIMEOUT` | | ✓ | | | | ✓ |
| `super::{ArtifactCtx, track_artifact}` | | ✓ | ✓ | | | ✓ |
| `crate::ai::filter::mask_sensitive` | | | ✓ | ✓ | | |
| `crate::daemon::session::FG_HOOK_COUNTER` | | | | ✓ | | |
| `crate::daemon::session::SessionStore` | ✓ | | | ✓ | ✓ | |
| `crate::daemon::session::append_session_message` | | | | ✓ | | |
| `crate::daemon::session::bg_done_subscribe` | | | | ✓ | | |
| `crate::daemon::utils::send_response_split` | | ✓ | | | | ✓ |
| `crate::daemon::utils::log_event` | | ✓ | ✓ | ✓ | | ✓ |
| `crate::daemon::utils::normalize_output` | | | | ✓ | | |
| `crate::ipc::Request` | | ✓ | | | | ✓ |
| `crate::ipc::Response` | | ✓ | | | | ✓ |
| `crate::ipc::RunbookListItem` | | ✓ | | | | |
| `crate::ipc::ScriptListItem` | | ✓ | | | | |
| `crate::scheduler::ScheduleStore` | | ✓ | | | | |
| `crate::scripts` | | ✓ | | | | |
| `crate::util::UnpoisonExt` | | | | ✓ | | |
| `std::sync::Arc` | | ✓ | | ✓ | | |
| `std::time::Duration` | | | | ✓ | | |

Notes on the partition (verified against the source):

- `crate::util::UnpoisonExt` is a **trait** imported for `.unwrap_or_log()` (old lines 623, 681, 682,
  all in `pane.rs`) — it shows up only as a method call, not a bare symbol, so do not drop it from
  `pane.rs`.
- `std::sync::Arc` is used in `artifacts.rs` (`delete_runbook`'s `&Arc<ScheduleStore>` param) and in
  `pane.rs` (`watch_pane`'s `Arc::clone`).
- `std::time::Duration` is used only in `pane.rs` (`watch_pane`). `agents.rs`'s `await_agent_result`
  uses **`tokio::time::Duration`** fully-qualified — that needs no `use` line.
- `list_memories` takes `_tx: &mut W` but never calls `send_response_split` — so `memory.rs` does
  **not** import `send_response_split`. Its `W: tokio::io::AsyncWriteExt + Unpin` bound is written
  fully-qualified, needing no `use`.
- `mask_sensitive` is used in `memory.rs` (`read_memory`) and `pane.rs` (`list_panes`, `watch_pane`).
- `spawn_ghost` keeps its two **function-body** `use` statements (`use crate::daemon::ghost::{…};`,
  `use crate::webhook::inject_ghost_event;`) verbatim inside the function — they are not top-level
  imports and do not appear in this table.

## Acceptance criteria

- [ ] `src/daemon/executor/knowledge.rs` no longer exists; `src/daemon/executor/knowledge/` contains
      `mod.rs`, `artifacts.rs`, `memory.rs`, `pane.rs`, `ghost.rs`, `agents.rs`.
- [ ] `src/daemon/executor/mod.rs` and `src/daemon/executor/foreground.rs` are **unchanged**:
      `git diff` is empty for both.
- [ ] **Fidelity (byte-for-byte content move):** the sorted multiset of non-blank, non-comment,
      whitespace-trimmed lines is identical before and after, modulo the authorized changes. After
      the split:
      ```sh
      cat src/daemon/executor/knowledge/*.rs | grep -vE '^\s*(//|$)' | sed 's/^[[:space:]]*//' | sort > /tmp/kn_after.txt
      diff /tmp/kn_before.txt /tmp/kn_after.txt
      ```
      The **only** permitted differences are:
      1. the added `mod …;` + `pub(super) use …;` re-export lines in `mod.rs`;
      2. the `use super::{ArtifactCtx, track_artifact};` cross-module imports in `artifacts.rs` /
         `memory.rs` / `agents.rs`;
      3. the `super::` → `super::super::` re-pathing of `ToolCallOutcome` / `USER_PROMPT_TIMEOUT`
         (the old `use super::…;` lines disappear; new `use super::super::…;` lines appear) and of
         the 4 `super::foreground::is_shell_prompt` call sites → `super::super::foreground::is_shell_prompt`;
      4. the per-file partitioning of the original top-level `use` lines (a grouped `use` splitting
         into per-file subsets is a rendering change of the same lines — note it);
      5. the 23 `pub(super)` → `pub` bumps on the re-exported leaf functions.
      No logic line, string literal, or `format!` template may appear, disappear, or change. (rustfmt
      may reflow a long line into a block; if so, note it and confirm it is rendering-only.) Paste the
      `diff` output in the Update Log and justify every line of it.
- [ ] Each new file is meaningfully smaller than the original 1341 lines and well under ~800:
      `mod.rs` ~45, `artifacts.rs` ~360, `memory.rs` ~220, `pane.rs` ~375, `ghost.rs` ~95,
      `agents.rs` ~280 (approximate).
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo fmt --all`, and `cargo test` all pass. **No new dependencies.**
- [ ] The full test suite passes with the **same test count as before** (no test was added, removed,
      or relocated — `knowledge.rs` has no test module).

## Test plan

No new tests. This is a verbatim move with no test module in the source file. All existing tests
across the suite must continue to pass unchanged — they reach this code through the unchanged
`knowledge::<Name>` re-export paths. Adding or removing a test, or changing an assertion, is out of
scope and a fidelity violation.

## End-to-end verification

Not applicable — phase ships no new runtime-loadable artifact (pure internal module reorganization).
The behavior is identical; the `cargo test` pass + the multiset-diff fidelity check are the
verification. State this line in the completion Update Log and paste the fidelity `diff` output.

## Authorizations

- [ ] May add dependencies: **none**.
- [ ] May NOT touch `docs/architecture.md`.
- [ ] May create `src/daemon/executor/knowledge/{mod,artifacts,memory,pane,ghost,agents}.rs` and
      delete `src/daemon/executor/knowledge.rs` (the split itself).
- [ ] May change visibility of exactly the 23 re-exported leaf functions named in §Visibility (a)
      (`pub(super)` → `pub`), and no others.

## Out of scope

- Any behavior change, refactor, rename, reordering, or "cleanup" of the moved code. Move it verbatim.
- Acting on the two pre-existing `// TODO(M2): consolidate params into a struct` comments (above
  `update_memory` and `create_agent`) — move them verbatim; do not consolidate.
- Splitting any other oversized file — this is the last C5 split in M2.
- Editing `src/daemon/executor/mod.rs` or `src/daemon/executor/foreground.rs` — the re-exports in the
  new `mod.rs` keep every consumer path identical, so they need no edit.
- Changing the visibility of `ArtifactCtx` (and its fields), `track_artifact`, or `WatchHookGuard`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-27 19:55 (started)

**By:** executor
**Action:** Began phase 15 split-knowledge. Created knowledge/ directory with mod.rs, artifacts.rs, memory.rs, pane.rs, ghost.rs, agents.rs. Deleted old knowledge.rs. All builds green.

### Update — 2026-06-27 19:58 (complete)

**By:** executor

**Summary:** Split `src/daemon/executor/knowledge.rs` (1341 lines) into `src/daemon/executor/knowledge/` directory with 6 files: `mod.rs` (49), `artifacts.rs` (357), `memory.rs` (217), `pane.rs` (366), `ghost.rs` (97), `agents.rs` (278). Total 1364 lines (45 new lines for mod declarations, re-exports, and per-file imports). Net behavior change = 0.

**Verification:**
- `cargo build`: zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings`: clean
- `cargo fmt --all`: clean
- `cargo test`: 26 passed, 1 failed (pre-existing `webhook_alert_to_event_log` — same failure on unmodified tree), 2 ignored. Same test count as before split.
- `git diff src/daemon/executor/mod.rs src/daemon/executor/foreground.rs`: empty (no edits to consumers)
- Fidelity diff: all differences are authorized — (1) mod declarations + re-exports in mod.rs, (2) `use super::{ArtifactCtx, track_artifact}` in 3 submodules, (3) `super::` → `super::super::` re-pathing of `ToolCallOutcome`/`USER_PROMPT_TIMEOUT` and 4 `is_shell_prompt` call sites, (4) per-file import partitioning, (5) 23 `pub(super)` → `pub` bumps on re-exported leaf functions. No logic lines changed.

**Files changed:**
- Created: `src/daemon/executor/knowledge/{mod,artifacts,memory,pane,ghost,agents}.rs`
- Deleted: `src/daemon/executor/knowledge.rs`
- Updated: `docs/dev/milestones/M2-tui-renderer/phase-15-split-knowledge.md` (status + Update Log)
- Updated: `docs/dev/milestones/M2-tui-renderer/README.md` (phase table)

**End-to-end verification:** N/A — phase ships no new runtime-loadable artifact (pure internal module reorganization). The behavior is identical; the `cargo test` pass + the multiset-diff fidelity check are the verification.

**Notes for review:** The `webhook_alert_to_event_log` integration test failure is pre-existing (present on the unmodified tree). The two `// TODO(M2): consolidate params into a struct` comments were moved verbatim as authorized.
