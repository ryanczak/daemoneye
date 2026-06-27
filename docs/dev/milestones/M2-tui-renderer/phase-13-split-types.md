# Phase 13: split-types

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** review
**Depends on:** none (independent C5 cleanup; touches only `src/ai/types.rs`)
**Estimated diff:** ~1413 lines moved (mechanical; net behavior change = 0)
**Tags:** language=rust, kind=refactor, size=l

> **Spec density: NORMAL (mechanical split).** This is a verbatim move-and-re-path split with
> no design discovery — the same shape as phases 04–06 / 08 / 09 / 12, all of which cleared
> first try. The layout, the symbol placement, the (minimal) re-pathing, and the re-exports are
> **fully pinned below**. Do not redesign, rename, reorder, or "improve" any code: move it
> verbatim and fix only the module paths. A byte-for-byte fidelity check (sorted-multiset line
> diff) is the acceptance gate.
>
> **This file is even simpler than phase 12 was.** Two facts make it so, both verified against
> the current source — rely on them, but let the compiler confirm:
> 1. **No `super::` re-pathing.** `src/ai/types.rs` has exactly one top-level `use`
>    (`use serde::{Deserialize, Serialize};`, an external crate — depth-independent). It
>    references **nothing** from its parent `ai` module via `super::` or `crate::`. So there is
>    no `super::` → `super::super::` rewrite to do (the gotcha that dominated phase 12). The
>    *only* new cross-module paths are two **internal sibling** references (see §Re-pathing).
> 2. **No visibility bumps.** Every moved item (`ToolCall`, `ToolResult`, `Message`,
>    `TokenBreakdown`, `PendingCall`, `AiEvent`) is already `pub`. The re-exports in `mod.rs`
>    are `pub use` of already-`pub` items, so there is **no E0364 visibility-widening** like
>    phase 12 hit. Do not add, remove, or change any visibility qualifier.

## Goal

Split the oversized `src/ai/types.rs` (1413 lines) into a `types/` submodule directory —
`wire`, `pending`, `events`, plus re-exports in `mod.rs` — to close part of code-issue C5
(oversized files). **No behavior change.** Every struct, enum, impl, and test moves verbatim to
its new home; only module paths change.

## Architecture references

Read before starting:

- `docs/ROADMAP.md` §2.2 **C5** (oversized files) — why this split exists.
- The prior C5 splits for the exact convention to mirror: `src/daemon/executor/file_ops/mod.rs`
  (phase 12) and `src/config/mod.rs` (phase 09) — a `mod.rs` that declares the (private)
  submodules and re-exports the public surface, submodules holding the moved code, tests
  co-located with the code they exercise.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc.
3. Confirm the repo is on a clean branch with no uncommitted changes.
4. Capture a fidelity baseline before touching anything (used in Acceptance):

   ```sh
   grep -vE '^\s*(//|$)' src/ai/types.rs | sed 's/^[[:space:]]*//' | sort > /tmp/types_before.txt
   wc -l /tmp/types_before.txt   # expect 1306
   ```

## Current state

`src/ai/types.rs` is one flat 1413-line file. It is a **sibling** of `src/ai/mod.rs`. Its only
top-level import is line 1:

```rust
use serde::{Deserialize, Serialize};
```

It declares no `mod`s and uses no `super::`/`crate::` paths anywhere (the custom deserializer
and `to_tool_call` use fully-qualified `serde::de::…` / `serde_json::…` inline, which are
depth-independent).

Its items (line numbers approximate):

| Item | Lines | Kind | Belongs in |
|---|---|---|---|
| `ToolCall` | 3–10 | `pub struct` (derives Serialize/Deserialize) | `wire.rs` |
| `ToolResult` | 12–17 | `pub struct` (derives Serialize/Deserialize) | `wire.rs` |
| `Message` | 19–34 | `pub struct` (derives Serialize/Deserialize) | `wire.rs` |
| `TokenBreakdown` | 41–55 | `pub struct` (derives Default/Serialize) | `wire.rs` |
| `impl TokenBreakdown` | 57–77 | `total`, `uncached_input_tokens` | `wire.rs` |
| `impl Deserialize for TokenBreakdown` | 80–147 | custom legacy-shape deserializer | `wire.rs` |
| `PendingCall` | 149–370 | `pub enum` (no derives) | `pending.rs` |
| `impl PendingCall` | 372–786 | `to_tool_call`, `id`, `should_emit_tool_feedback`, `summary`, `tool_name` | `pending.rs` |
| `AiEvent` | 788–1001 | `pub enum` (`#[derive(Debug)]`) | `events.rs` |
| `#[cfg(test)] mod tests` | 1003–1413 | tests | split per §Test placement |

**External consumers** reach these types as `crate::ai::types::<Name>` (verified — all 9 call
sites): `src/cost.rs`, `src/daemon/digest.rs` (`ToolCall`), `src/ai/backends/{openai,gemini,
anthropic}.rs` (`AiEvent`, `Message`, `TokenBreakdown`, and `ToolCall` in an anthropic test),
`src/ai/tools/{args,dispatch}.rs` (`AiEvent`). In addition, `src/ai/mod.rs:14` re-exports the
surface upward:

```rust
pub use types::{AiEvent, Message, PendingCall, TokenBreakdown, ToolResult};
```

Both of these access patterns go through the `types` module's public surface, so **every name
above must remain reachable as `crate::ai::types::<Name>`** after the split. The re-exports in
the new `mod.rs` (§Spec item 1) are what preserve this. `src/ai/mod.rs:14` must stay
**byte-for-byte unchanged**.

**Cross-item references that dictate the two internal sibling imports** (verified):

- `PendingCall::to_tool_call` (in `pending.rs`) returns a `ToolCall` (defined in `wire.rs`).
- `AiEvent::Done(TokenBreakdown)` (in `events.rs`) names `TokenBreakdown` (defined in `wire.rs`).

Nothing else crosses between the three submodules.

## Spec

Create `src/ai/types/` and delete the flat `types.rs`. `ai/mod.rs`'s `pub mod types;`
declaration (line 4) is unchanged — Rust resolves `types` to either `types.rs` or
`types/mod.rs`.

Land each sub-deliverable and `cargo build`-green before the next.

1. **`types/mod.rs` — submodule declarations + re-exports.** Create it with exactly:

   ```rust
   mod events;
   mod pending;
   mod wire;

   pub use events::AiEvent;
   pub use pending::PendingCall;
   pub use wire::{Message, TokenBreakdown, ToolCall, ToolResult};
   ```

   The submodules are **private** (`mod`, not `pub mod`) — consumers use the re-exported
   `crate::ai::types::<Name>` paths, never `crate::ai::types::wire::<Name>`. The `pub use`
   re-exports of already-`pub` items widen nothing (no E0364): they simply surface the names at
   the `types` level, which is exactly where every consumer and `ai/mod.rs:14` expects them.
   `mod.rs` needs **no** `use` lines of its own (it defines no code, only declares + re-exports).

2. **`types/wire.rs`** — move verbatim: `ToolCall`, `ToolResult`, `Message`, `TokenBreakdown`,
   `impl TokenBreakdown`, `impl Deserialize for TokenBreakdown`, plus the wire-related tests
   (see §Test placement). Keep the top-level `use serde::{Deserialize, Serialize};` here (these
   types are the only `Serialize`/`Deserialize` derivers). The custom deserializer's inner
   `use serde::de::{MapAccess, Visitor};` moves with it verbatim.

3. **`types/pending.rs`** — move verbatim: `PendingCall` and its `impl` block, plus the
   pending-related tests (§Test placement). Add the one sibling import it needs (§Re-pathing).

4. **`types/events.rs`** — move verbatim: `AiEvent`. No tests move here. Add the one sibling
   import it needs (§Re-pathing).

5. **Delete** the old `src/ai/types.rs`.

### Visibility — no changes

Every moved item is already `pub` and stays `pub`. Unlike phase 12, **no `pub(super)` bumps and
no `pub`-widening are required** — the re-exports in §Spec item 1 are `pub use` of items that are
already `pub`, which compiles cleanly. Adding or changing any visibility qualifier is an
unrequested change and will bounce.

### Re-pathing — only two internal sibling imports

There is **no** `super::` → `super::super::` rewrite (the file uses no parent-module paths).
The only new paths are the two internal sibling references:

- In **`pending.rs`**: `PendingCall::to_tool_call` returns `ToolCall`. Add
  `use super::wire::ToolCall;` (or equivalently `use super::ToolCall;` through the mod.rs
  re-export — either resolves; prefer the direct `super::wire::ToolCall`).
- In **`events.rs`**: `AiEvent::Done(TokenBreakdown)` names `TokenBreakdown`. Add
  `use super::wire::TokenBreakdown;` (or `use super::TokenBreakdown;`).

`serde_json::…` and `serde::de::…` usages inside method/impl bodies are fully-qualified and
depth-independent — copy them unchanged. Let the compiler flag any missing/unused import.

### Test placement

Co-locate each test with the code it exercises (STANDARDS §2.5). Move them verbatim — same
names, same assertions. Each submodule's `#[cfg(test)] mod tests` opens with `use super::*;`
(as today).

- **→ `wire.rs`** (`mod tests`): `message_roundtrip_plain`, `message_tool_calls_skipped_when_none`,
  `tool_call_roundtrip`, `token_breakdown_total_sums_all_buckets`,
  `token_breakdown_zero_tokens_is_zero`,
  `token_breakdown_uncached_input_tokens_returns_input_field`,
  `token_breakdown_serializes_all_fields`,
  `legacy_ai_usage_jsonl_deserializes_into_token_breakdown`,
  `token_breakdown_new_format_deserializes_directly`,
  `token_breakdown_zero_cache_when_provider_omits_field`. The `user_msg` test helper (builds a
  `Message`) moves here.
- **→ `pending.rs`** (`mod tests`): `should_emit_tool_feedback_silent_tools_true`,
  `should_emit_tool_feedback_approval_gated_tools_false`, `summary_read_file_path_only`,
  `summary_read_file_with_offset_and_limit`, `summary_read_file_with_grep`,
  `summary_watch_pane_with_pattern`, `summary_watch_pane_no_pattern`,
  `summary_search_repository_truncated`, `summary_memory_key_category`,
  `summary_list_memories_all`, `summary_list_memories_category`, `summary_spawn_ghost`,
  `summary_get_terminal_context_empty`. The `mk_read_file` and `mk_foreground` test helpers
  (build `PendingCall`s) move here.
- **→ `events.rs`**: no tests (no current test exercises `AiEvent` in isolation).

No test helper is shared across submodules (`user_msg` is wire-only; `mk_read_file` /
`mk_foreground` are pending-only), so no duplication or shared-test-location decision is needed.

## Acceptance criteria

- [ ] `src/ai/types.rs` no longer exists; `src/ai/types/` contains `mod.rs`, `wire.rs`,
      `pending.rs`, `events.rs`.
- [ ] `src/ai/mod.rs` is **unchanged** — in particular line 4 (`pub mod types;`) and line 14
      (`pub use types::{AiEvent, Message, PendingCall, TokenBreakdown, ToolResult};`). Verify
      with `git diff src/ai/mod.rs` — expect no diff.
- [ ] **Fidelity (byte-for-byte content move):** the sorted multiset of non-blank, non-comment,
      whitespace-trimmed lines is identical before and after. After the split:
      ```sh
      cat src/ai/types/*.rs | grep -vE '^\s*(//|$)' | sed 's/^[[:space:]]*//' | sort > /tmp/types_after.txt
      diff /tmp/types_before.txt /tmp/types_after.txt
      ```
      The **only** permitted differences are: the added `mod events; mod pending; mod wire;` and
      `pub use …;` re-export lines in `mod.rs`; and the two added sibling `use super::wire::…`
      import lines (§Re-pathing). No `pub`/visibility line, logic line, string literal, or
      test-assertion line may appear, disappear, or change. (rustfmt may reflow a long line into
      a block; if so, note it and confirm it is rendering-only.) Paste the `diff` output in the
      Update Log and justify every line of it.
- [ ] Each new file is meaningfully smaller than the original 1413 lines. `wire.rs` and
      `events.rs` land well under 800; **`pending.rs` is expected to land ~900 lines** — that is
      acceptable and authorized: it is a single cohesive enum plus its inherent `impl` (the five
      match-based methods), which cannot be split further without separating an enum from its own
      methods. This is the same "irreducible single-item file" exception documented for
      `tools/defs.rs` in phase 07. Do **not** invent a fourth submodule to shrink it.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets --all-features -- -D
      warnings`, `cargo fmt --all`, and `cargo test` all pass. **No new dependencies.**
- [ ] The same set of tests runs and passes as before (the `message_*`, `tool_call_roundtrip`,
      `token_breakdown_*`, `legacy_ai_usage_*`, `should_emit_tool_feedback_*`, `summary_*`
      tests) — same names, same count, relocated per §Test placement.

## Test plan

No new tests. This is a verbatim move: all existing `types` unit tests must continue to pass
under their original names, relocated to the submodule that owns the code they exercise. Adding
or removing a test, or changing an assertion, is out of scope and a fidelity violation.

## End-to-end verification

Not applicable — phase ships no new runtime-loadable artifact (pure internal module
reorganization). The behavior is identical; the `cargo test` pass + the multiset-diff fidelity
check are the verification. State this line in the completion Update Log and paste the fidelity
`diff` output.

## Authorizations

- [ ] May add dependencies: **none**.
- [ ] May NOT touch `docs/architecture.md`.
- [ ] May create `src/ai/types/{mod,wire,pending,events}.rs` and delete `src/ai/types.rs`
      (the split itself).
- [ ] No visibility changes are authorized (none are needed — see §Visibility).

## Out of scope

- Any behavior change, refactor, rename, reordering, or "cleanup" of the moved code. Move it
  verbatim.
- Splitting any other oversized file — `daemon/background.rs` (phase 14),
  `daemon/executor/knowledge.rs` (phase 15).
- Changing `src/ai/mod.rs` in any way (the re-exports in the new `types/mod.rs` keep every
  consumer path identical, so `ai/mod.rs` needs no edit).
- Touching the backends (`ai/backends/*.rs`), `ai/tools/*`, `cost.rs`, or `daemon/digest.rs` —
  the re-exports keep their `crate::ai::types::<Name>` imports byte-for-byte valid.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-27 15:43 (started)

**Executor:** rexyMCP executor

Split `src/ai/types.rs` (1413 lines) into `src/ai/types/` directory with `mod.rs`, `wire.rs`,
`pending.rs`, `events.rs`. Verbatim move with two sibling imports added.

### Update — 2026-06-27 15:45 (complete)

**Executor:** rexyMCP executor

**Verification commands:**
- `cargo build`: zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings`: clean
- `cargo fmt --all`: clean
- `cargo test --lib`: 814 passed, 0 failed
- `cargo test` (integration): 27 passed, 0 failed, 2 ignored

**Files changed:**
- `src/ai/types.rs` → deleted (1413 lines)
- `src/ai/types/mod.rs` → created (6 lines: submodule declarations + re-exports)
- `src/ai/types/wire.rs` → created (277 lines: ToolCall, ToolResult, Message, TokenBreakdown + impls + 10 tests)
- `src/ai/types/pending.rs` → created (927 lines: PendingCall + impl + 13 tests)
- `src/ai/types/events.rs` → created (216 lines: AiEvent)
- `src/ai/mod.rs`: unchanged (verified with `git diff`)

**Fidelity diff** (`diff /tmp/types_before.txt /tmp/types_after.txt`):
```
86a87
> }
367a369
> #[cfg(test)]
729a732,734
> mod events;
> mod pending;
> mod tests {
730a736
> mod wire;
1032a1304,1306
> pub use events::AiEvent;
> pub use pending::PendingCall;
> pub use wire::{Message, TokenBreakdown, ToolCall, ToolResult};
1294a1304,1306
> use super::*;
> use super::wire::TokenBreakdown;
> use super::wire::ToolCall;
```
All added lines are exactly the authorized additions: `mod.rs` submodule declarations and re-exports, two sibling `use super::wire::…` imports, and test module wrappers (`#[cfg(test)]`, `mod tests {`, `use super::*`, `}`). No logic, visibility, or assertion lines changed.

**Grep for spec-pinned literals:**
```
grep 'use super::wire::ToolCall' src/ai/types/pending.rs → line 1 ✓
grep 'use super::wire::TokenBreakdown' src/ai/types/events.rs → line 1 ✓
```

**End-to-end verification:** N/A — phase ships no new runtime-loadable artifact (pure internal module reorganization). The `cargo test` pass + multiset-diff fidelity check are the verification.

**Notes for review:** None — mechanical split, no adaptations needed.
