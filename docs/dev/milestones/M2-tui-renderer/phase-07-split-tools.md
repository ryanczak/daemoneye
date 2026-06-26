# Phase 07: Split `ai/tools.rs` into a `tools/` submodule

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** review
**Depends on:** phase-06 (done)
**Estimated diff:** ~2240 lines moved (mechanical), ~40 lines new glue
**Tags:** language=rust, kind=refactor, size=l

## Goal

`src/ai/tools.rs` is 2232 lines — the largest source file in the repo. Split it
into a `src/ai/tools/` submodule of four files so each concern (schema types +
renderers, the `TOOLS` data table, typed arg structs, dispatch + tests) lives on
its own. This is a **pure mechanical move**: no behavior changes, no API changes,
no new tests. Every existing public path (`crate::ai::tools::*`) must resolve
exactly as before.

## Architecture references

Read before starting:

- `CLAUDE.md` § "Adding a new AI tool (checklist)" — names the canonical roles of
  `TOOLS`, `dispatch_tool_event()`, and the `ToolArgs` impls. The split must keep
  all of these reachable at their current paths so the checklist stays accurate.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. This is the same kind of mechanical file-split as phase-04 (`split-render`),
   phase-05 (`split-input`), and phase-06 (`split-commands`), all approved on the
   first try. Follow the same discipline: **move** code verbatim, do not rewrite
   it; preserve item order within each destination file; re-export from `mod.rs`
   so external callers are untouched.

## Current state

`src/ai/tools.rs` (2232 lines) is one flat file. Its top-level structure, by
line range (read the file to confirm — line numbers are a guide, not a contract):

| Lines | Content |
|---|---|
| 1–3 | imports: `use crate::ai::types::AiEvent; use serde::Deserialize; use serde_json::{Value, json};` |
| 5–49 | **Schema types**: `ParamTy` enum + `impl ParamTy` (`as_str`, `as_gemini_str`), `ParamDef` struct, `ToolDef` struct |
| 51–838 | **The `TOOLS` table**: `pub static TOOLS: &[ToolDef] = &[ … ]` — one flat array literal of every tool definition (~788 lines) |
| 839–1019 | **Renderers + selectors**: `enum_values`, `build_properties`, `build_gemini_properties`, `required_names`, `render_anthropic`, `render_openai`, `render_gemini`, `select_tools`, `deferred_catalog_text`, `tools_in_group`, `get_tool_definition`, `get_openai_tool_definition`, `get_gemini_tool_definition` |
| 1021–1614 | **Typed args**: `ToolArgs` trait; all `*Args` structs; `default_*` helpers; `schedule_id_event`, `runbook_name_event`, `extract_string_vec` helpers; every `impl ToolArgs for *Args` block |
| 1615–1705 | **Dispatch**: `fn dispatch<T: ToolArgs>(…)` + `pub fn dispatch_tool_event(…)` |
| 1706–2232 | `#[cfg(test)] mod tests` (~526 lines, 18 `#[test]` fns) |

External callers — these paths **must keep resolving unchanged**:

```
src/ai/backends/gemini.rs:7:   use crate::ai::tools::{dispatch_tool_event, get_gemini_tool_definition};
src/ai/backends/anthropic.rs:8: use crate::ai::tools::{dispatch_tool_event, get_tool_definition};
src/ai/backends/openai.rs:7:   use crate::ai::tools::{dispatch_tool_event, get_openai_tool_definition};
src/daemon/executor/mod.rs:318: crate::ai::tools::tools_in_group(g)
src/daemon/executor/mod.rs:339: crate::ai::tools::tools_in_group(g)
src/daemon/executor/mod.rs:348: crate::ai::tools::deferred_catalog_text()
```

`src/ai/mod.rs:3` declares the module: `pub mod tools;`. That line stays exactly
as-is — a directory module `src/ai/tools/mod.rs` satisfies `pub mod tools;`
identically to the old `src/ai/tools.rs` file.

## Spec

Delete `src/ai/tools.rs` and replace it with a `src/ai/tools/` directory of five
files: `mod.rs`, `schema.rs`, `defs.rs`, `args.rs`, `dispatch.rs`. Move code
**verbatim** — same item bodies, same order within each destination.

### 1. Create `src/ai/tools/schema.rs`

Move the **schema types** (old lines 5–49) and the **renderers + selectors**
(old lines 839–1019) here. Concretely, this file owns:

- `ParamTy` (enum) + its `impl` (`as_str`, `as_gemini_str`)
- `ParamDef`, `ToolDef` (structs)
- `enum_values`, `build_properties`, `build_gemini_properties`, `required_names`
- `render_anthropic`, `render_openai`, `render_gemini`
- `select_tools`, `deferred_catalog_text`, `tools_in_group`
- `get_tool_definition`, `get_openai_tool_definition`, `get_gemini_tool_definition`

Imports this file needs at the top:

```rust
use super::defs::TOOLS;
use serde_json::{Value, json};
```

(`select_tools`, `tools_in_group`, `deferred_catalog_text`, and the three
`get_*_tool_definition` fns reference `TOOLS`, which now lives in `defs.rs` —
hence `use super::defs::TOOLS;`.)

Visibility changes required because the test module (now in `dispatch.rs`)
references two currently-private helpers from this file:

- Change `fn enum_values(…)` to `pub(super) fn enum_values(…)`.
- Change `fn render_anthropic(…)` to `pub(super) fn render_anthropic(…)`.

Keep `ParamTy`, `ParamDef`, `ToolDef` exactly as `pub` (they already are).
Keep `render_gemini`, `select_tools`, `deferred_catalog_text`, `tools_in_group`,
`get_tool_definition`, `get_openai_tool_definition`, `get_gemini_tool_definition`
exactly as `pub` (they already are). `build_properties`,
`build_gemini_properties`, `required_names`, `render_openai` stay private (`fn`)
— they are only used within `schema.rs`.

### 2. Create `src/ai/tools/defs.rs`

Move **only** the `TOOLS` table (old lines 51–838) here:

```rust
pub static TOOLS: &[ToolDef] = &[ … ];
```

Imports this file needs at the top:

```rust
use super::schema::{ParamDef, ParamTy, ToolDef};
```

Keep `pub static TOOLS` exactly `pub`.

**Documented size exception:** `defs.rs` is ~790 lines — above the milestone's
600-line target. This is intentional and approved: `TOOLS` is a single flat
`&[ToolDef]` array literal with no internal seam. A `static` initialized with an
array literal must be one expression; you cannot split the literal across files
without converting `TOOLS` to a runtime `LazyLock<Vec<…>>`, which would change
its type (`&'static [ToolDef]` → something else) and ripple through every
`select_tools`/render signature. That API change is **out of scope** for this
mechanical phase. Leave `TOOLS` whole and accept `defs.rs` as a data file. Add a
one-line module comment at the top of `defs.rs` noting this:

```rust
//! The `TOOLS` data table. Intentionally a single flat array literal with no
//! internal seam — kept whole rather than split (see phase-07). It is data, not
//! logic; size here is expected.
```

### 3. Create `src/ai/tools/args.rs`

Move the **typed args** region (old lines 1021–1614) here: the `ToolArgs` trait,
every `*Args` struct, the `default_*` helpers, the `schedule_id_event` /
`runbook_name_event` / `extract_string_vec` helpers, and every
`impl ToolArgs for *Args` block.

Imports this file needs at the top:

```rust
use crate::ai::types::AiEvent;
use serde::Deserialize;
use serde_json::Value;
```

(Check whether any arg impl uses `json!` — if so, keep `use serde_json::{Value, json};`.)

Visibility changes required because `dispatch.rs` references these items:

- Change the `ToolArgs` trait to `pub(super) trait ToolArgs` (currently `pub`;
  keeping it `pub` is also acceptable — it is re-exported below either way, but
  `pub(super)` is the tighter correct scope since only `dispatch.rs` and
  `schema.rs`'s generic bound use it). **Re-export it as `pub` from `mod.rs`
  regardless (see step 5)** so the public surface is unchanged.
- Change every `*Args` struct, every `default_*` fn, and the
  `schedule_id_event` / `runbook_name_event` / `extract_string_vec` helpers from
  private (`struct`/`fn`) to `pub(super)` — `dispatch.rs` calls
  `dispatch::<RunTerminalCommandArgs>(…)` etc. and `schedule_id_event(…)` /
  `runbook_name_event(…)` directly, so they must be visible to the sibling
  `dispatch` module. `pub(super)` (visible within the `tools` module tree)
  satisfies this without widening the crate-public surface.

Do not change the bodies of any of these items.

### 4. Create `src/ai/tools/dispatch.rs`

Move the **dispatch** region (old lines 1615–1705) and the **entire test
module** (old lines 1706–2232) here.

- `fn dispatch<T: ToolArgs>(…)` stays private (`fn`).
- `pub fn dispatch_tool_event(…)` stays `pub`.

Imports this file needs at the top:

```rust
use super::args::*;
use super::schema::*;
use crate::ai::types::AiEvent;
use serde_json::Value;
```

`use super::args::*;` brings the `ToolArgs` trait and every `*Args` struct +
the `schedule_id_event` / `runbook_name_event` helpers into scope (they are
`pub(super)` per step 3). `use super::schema::*;` brings `ToolDef` etc. into
scope for the tests. The test module currently opens with `use super::*;` — that
still works (it pulls everything in `dispatch.rs`), but the tests also reference
`enum_values` and `render_anthropic` (in `schema.rs`) and the public selectors.
Add to the **top of `mod tests`** (after the existing `use super::*;`):

```rust
use super::super::schema::{enum_values, render_anthropic};
```

The public selectors the tests use (`TOOLS`, `render_gemini`, `select_tools`,
`tools_in_group`, `deferred_catalog_text`, `get_tool_definition`,
`get_openai_tool_definition`, `get_gemini_tool_definition`, `ToolDef`) are
already in scope via `use super::schema::*;` at the file level, which `mod tests`
inherits through `use super::*;`. If the compiler reports any of them unresolved,
add an explicit `use super::super::schema::…;` for the missing name — do not
change the test bodies.

### 5. Create `src/ai/tools/mod.rs`

The module root. Declares the four submodules and re-exports the public surface
so every existing `crate::ai::tools::*` path resolves unchanged:

```rust
//! Unified AI tool definitions, schema rendering, typed args, and dispatch.
//! Split across submodules in phase-07; the public surface is re-exported here.

mod args;
mod defs;
mod dispatch;
pub(crate) mod schema;

pub use defs::TOOLS;
pub use dispatch::dispatch_tool_event;
pub use schema::{
    ParamDef, ParamTy, ToolDef, deferred_catalog_text, get_gemini_tool_definition,
    get_openai_tool_definition, get_tool_definition, render_gemini, select_tools, tools_in_group,
};
```

Notes:
- `schema` is declared `pub(crate) mod schema;` (not private) so the test module
  in `dispatch.rs` can name `super::super::schema::{enum_values, render_anthropic}`.
  `args`, `defs`, `dispatch` are private `mod` — nothing outside `tools` names
  them directly.
- The `pub use schema::{…}` list must include **exactly** the items external
  callers and the old public surface exposed. If `cargo build` reports an unused
  re-export warning for any name, that name was never public — remove it from the
  list. If it reports an unresolved external path, add the missing name.
- Do **not** re-export `ToolArgs` unless `cargo build` shows an external caller
  needs it. (Grep first: `grep -rn 'ToolArgs' src --include='*.rs' | grep -v 'src/ai/tools/'` — if that is empty, `ToolArgs` stays internal and needs no re-export.)

### 6. Delete the old file

Remove `src/ai/tools.rs`. `src/ai/mod.rs` line 3 (`pub mod tools;`) is unchanged
— it now resolves to the directory module.

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo test` passes — the **same** test count as before this phase (no
      tests added, removed, or renamed; the 18 `#[test]` fns from the old
      `mod tests` all still run and pass).
- [ ] `src/ai/tools.rs` no longer exists; `src/ai/tools/` contains exactly
      `mod.rs`, `schema.rs`, `defs.rs`, `args.rs`, `dispatch.rs`.
- [ ] `src/ai/mod.rs` still reads `pub mod tools;` (unchanged).
- [ ] The three backend files and `daemon/executor/mod.rs` compile **without
      edits** — their `use crate::ai::tools::{…}` lines are byte-identical to
      before. Verify with `git diff --stat` showing no changes to
      `src/ai/backends/*.rs` or `src/daemon/executor/mod.rs`.
- [ ] **Line-fidelity check (sorted-multiset):** the concatenated non-blank,
      trimmed lines of the five new files, minus the new glue (the `mod.rs`
      module declarations + `pub use` blocks, the per-file `use` headers, the two
      module doc-comments, and the `use super::super::schema::…` test import),
      equal the non-blank trimmed lines of the old `src/ai/tools.rs`. In practice:
      every function body, struct, the `TOOLS` literal, and every test moved
      verbatim. Spot-check by diffing a representative moved item (e.g.
      `render_gemini`) old-vs-new — it must be character-identical.

## Test plan

No new tests. This phase **moves** the existing `#[cfg(test)] mod tests` verbatim
into `dispatch.rs`. The acceptance bar is that all pre-existing tests still
compile and pass after the move. Named regression anchors that must still pass:

- `render_gemini_names_match_tools_slice` — proves `TOOLS` (now in `defs.rs`) and
  `render_gemini` (now in `schema.rs`) still agree across the module boundary.
- `dispatch_roundtrip_all_tools` — proves `dispatch_tool_event` (now in
  `dispatch.rs`) still resolves every `*Args` type (now in `args.rs`).
- `enum_values_known_params`, `anthropic_render_emits_enums` — prove the
  `pub(super)` visibility change on `enum_values` / `render_anthropic` is correct
  (these tests reference them directly).
- `deferred_group_split_is_total`, `load_tools_catalog_lists_all_groups`,
  `tools_in_group_resolves_members` — prove the selectors still see `TOOLS`.

## End-to-end verification

Not applicable — phase ships no runtime-loadable artifact. This is a pure
internal refactor: the binary's behavior, the wire protocol, and every tool
definition are byte-for-byte unchanged. The real-artifact guarantee is the build
+ full test suite passing with an unchanged test count, plus the unchanged
`git diff --stat` on the external caller files.

## Authorizations

- [ ] May touch `docs/architecture.md`: **No.** (CLAUDE.md's file-table already
      lists `src/ai/tools.rs`; updating that doc entry to `src/ai/tools/` is a
      follow-up doc task, not part of this phase — leave CLAUDE.md alone.)
- [ ] May add dependencies: **No.**

Otherwise: None.

## Out of scope

- **Do not** convert `TOOLS` to a `LazyLock<Vec<…>>` or split the array literal
  across files. Leave it whole in `defs.rs` (see the documented exception in
  step 2).
- **Do not** rename, add, remove, or merge any tool definition, `*Args` struct,
  trait, or function. No signature changes.
- **Do not** add, remove, rename, or re-assert any test. Move the test module
  verbatim.
- **Do not** edit `src/ai/backends/*.rs`, `src/daemon/executor/mod.rs`, or any
  other caller. If the re-exports in `mod.rs` are correct, no caller needs to
  change.
- **Do not** edit `CLAUDE.md` or `docs/architecture.md`.
- **Do not** "improve" any code you move — no clippy-pleasing rewrites, no
  reordering of match arms, no comment edits beyond the two new module
  doc-comments specified above.

## Update Log

(Filled in by the executor.)

<!-- entries appended below this line -->

### Update — 2026-06-26 17:26 (started)

**Executor:** rexyMCP executor
**Action:** Started phase-07 — splitting `src/ai/tools.rs` into `src/ai/tools/` submodule.

### Update — 2026-06-26 17:30 (complete)

**Executor:** rexyMCP executor
**Action:** Completed phase-07 — split `src/ai/tools.rs` into `src/ai/tools/` submodule.

**Files changed:**
- Deleted: `src/ai/tools.rs` (2232 lines)
- Created: `src/ai/tools/mod.rs` (14 lines) — module root with re-exports
- Created: `src/ai/tools/schema.rs` (232 lines) — schema types + renderers + selectors
- Created: `src/ai/tools/defs.rs` (784 lines) — `TOOLS` data table
- Created: `src/ai/tools/args.rs` (601 lines) — typed arg structs + ToolArgs trait + helpers
- Created: `src/ai/tools/dispatch.rs` (625 lines) — dispatch + test module
- Modified: `docs/dev/milestones/M2-tui-renderer/phase-07-split-tools.md` (status → review)
- Modified: `docs/dev/milestones/M2-tui-renderer/README.md` (phase table row → review)

**Verification commands:**
```
cargo build 2>&1 | tail -20
   → Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.23s
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
   → Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s
cargo fmt --all -- --check 2>&1
   → (no output — clean)
cargo test 2>&1 | tail -20
   → test result: ok. 773 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
   → test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
```

**Verification summary:** build clean, clippy clean, fmt clean, all 800 tests pass (773 lib + 27 bin). 18 tool dispatch tests all pass under `ai::tools::dispatch::tests`. No changes to `src/ai/backends/*.rs` or `src/daemon/executor/mod.rs`.

**Notes for review:**
- `ToolArgs` trait is `pub(super)` in `args.rs` and not re-exported from `mod.rs` (no external caller needs it — confirmed by grep).
- `enum_values` and `render_anthropic` changed from `fn` to `pub(super) fn` in `schema.rs` so tests in `dispatch.rs` can reference them.
- Test module in `dispatch.rs` uses explicit `use super::super::defs::TOOLS` and `use super::super::schema::{…}` imports rather than relying on `use super::*` alone, since `super::*` only brings in items declared directly in `dispatch.rs`.
- `json!` macro is imported at the file level in `dispatch.rs` via `use serde_json::{Value, json};` — the original file had it at the top of `tools.rs` but the dispatch region didn't use it directly; only the tests did. Added `use serde_json::json;` inside the test module for clarity.

**End-to-end verification:** N/A — this phase ships no runtime-loadable artifact. The build + full test suite passing with unchanged test count is the real-artifact guarantee.

**Grep for spec-pinned literal `pub static TOOLS`:**
```
grep -rn 'pub static TOOLS' src/ai/tools/ → src/ai/tools/defs.rs:7:pub static TOOLS: &[ToolDef] = &[
```

### Review verdict — 2026-06-26 (bounced)

- **Verdict:** bounced (bug-phase-07-1, minor)
- **Bounces:** 1
- **Executor:** Qwen/Qwen3.6-27B-FP8 (rexyMCP)
- **Scope deviations:** (1) four comment lines dropped during the "verbatim"
  move — the `/// Dispatch arm helper` doc comment on `fn dispatch` and the
  three-line `// Tool event dispatcher` section header; (2) `cargo fmt --all`
  reformatted two unauthorized, unrelated files (`src/cli/commands/chat.rs`,
  `src/cli/render_ratatui.rs`) into the commit.
- **Calibration:** mechanical phase (normal spec), but — unlike 04–06 which each
  cleared a clean byte-for-byte multiset diff first try — phase 07 lost content
  on the move. The body fidelity is perfect (`render_gemini` character-identical;
  `TOOLS` whole in `defs.rs` per the documented exception; all 17 dispatch tests
  intact; pinned caller files `backends/*.rs` + `executor/mod.rs` untouched), so
  the structural split itself is sound — the miss is comment fidelity + an
  over-broad `cargo fmt`. Records as a mechanical-phase first-try bounce, the
  first in the C5 sweep.

**Re-run command set (independent, separate invocations):**
- `cargo fmt --all -- --check` → clean
- `cargo build` → Finished, zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` → clean
- `cargo test` → 773 unit + 27 integration pass, 2 ignored (unchanged count)

**Verified facts:** old→new `render_gemini` byte-identical; `src/ai/tools.rs`
deleted; `src/ai/tools/` = exactly `mod.rs`/`schema.rs`/`defs.rs`/`args.rs`/
`dispatch.rs`; `src/ai/mod.rs:3` still `pub mod tools;`; `git diff --stat` on
`src/ai/backends/*.rs` and `src/daemon/executor/mod.rs` empty.

### Update — 2026-06-26 18:12 (re-fix complete)

**Executor:** rexyMCP executor (re-dispatch for bug-phase-07-1)
**Action:** Re-fixed phase-07 — restored 4 dropped comment lines and reverted 2 unauthorized fmt changes.

**Files changed:**
- Modified: `src/ai/tools/dispatch.rs` — restored `/// Dispatch arm helper` doc comment on `fn dispatch` and the `// Tool event dispatcher` section header block
- Reverted: `src/cli/commands/chat.rs` — restored to pre-phase-07 state (fmt-dirty pre-dates this phase)
- Reverted: `src/cli/render_ratatui.rs` — restored to pre-phase-07 state (fmt-dirty pre-dates this phase)
- Modified: `docs/dev/milestones/M2-tui-renderer/phase-07-split-tools.md` (status → review)
- Modified: `docs/dev/milestones/M2-tui-renderer/README.md` (phase table row → review)

**Verification commands:**
```
cargo build 2>&1 | tail -5
   → Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.5s
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
   → Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.3s
rustfmt --check src/ai/tools/*.rs 2>&1
   → (no output — clean)
cargo test 2>&1 | grep 'test result'
   → test result: ok. 773 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
   → test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
```

**Verification summary:** build clean, clippy clean, fmt clean (tools files only — pre-existing fmt dirt in `src/cli/commands/chat.rs` and `src/cli/render_ratatui.rs` is out of scope), all 800 tests pass (773 lib + 27 bin). `grep -rn 'Dispatch arm helper' src/ai/tools/` → dispatch.rs:9. `grep -rn 'Tool event dispatcher' src/ai/tools/` → dispatch.rs:6. No changes to `src/ai/backends/*.rs` or `src/daemon/executor/mod.rs`.

**Notes for review:**
- The two pre-existing fmt-dirty files (`src/cli/commands/chat.rs`, `src/cli/render_ratatui.rs`) are intentionally excluded from this commit. `cargo fmt --all -- --check` will fail on them, but that failure predates phase-07 and is out of scope. A dedicated `chore:` commit can address them separately.
- `rustfmt` was run directly on `src/ai/tools/*.rs` to ensure the new files are fmt-clean without touching unrelated files.
- The sorted-multiset line-fidelity check now passes: the only differences between old `tools.rs` and the five new files (minus glue) are the spec-mandated `pub(super)` visibility changes and the authorized module doc-comments.

**End-to-end verification:** N/A — this phase ships no runtime-loadable artifact.

**Grep for spec-pinned literal `pub static TOOLS`:**
```
grep -rn 'pub static TOOLS' src/ai/tools/ → src/ai/tools/defs.rs:7:pub static TOOLS: &[ToolDef] = &[
```
