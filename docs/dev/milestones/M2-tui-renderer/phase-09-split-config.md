# Phase 09: Split `config.rs` into a `config/` submodule

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** review
**Depends on:** phase-08 (done)
**Estimated diff:** ~1631 lines moved (mechanical), ~40 lines new glue
**Tags:** language=rust, kind=refactor, size=l

## Goal

`src/config.rs` is 1631 lines — the third-largest source file in the repo and
the next target in the C5 oversized-file sweep. Split it into a
`src/config/` submodule of four files so each concern lives on its own:
the config struct/enum definitions, the load/resolve/validate logic + path
helpers, the seeding functions + embedded asset constants, and the module root
that re-exports the public surface and holds the test suite. This is a **pure
mechanical move**: no behavior changes, no API changes, no new tests. Every
existing public path (`crate::config::*`) must resolve exactly as before.

This is the same kind of split as phase-04/05/06/07/08. Phase-07 bounced once
(dropped four comment lines during a "verbatim" move); phase-08 cleared on the
first try by pre-injecting that lesson. **This phase has one additional hazard
the earlier splits did not: `include_str!` relative paths.** `config.rs` has
**14** `include_str!("../assets/…")` calls. When code moves from
`src/config.rs` to a file one directory deeper (`src/config/seeds.rs`), every
one of those relative paths must gain a `../`. This is the headline gotcha —
read Pre-flight 6 and Spec § "The `include_str!` path adjustment" before
touching any code. Get this wrong and the build fails with
`couldn't read … No such file or directory`.

## Architecture references

Read before starting:

- `CLAUDE.md` § "Key files" — the table row for `src/config.rs` names its
  canonical role: "`~/.daemoneye/config.toml` parsing; `SRE_PROMPT_TOML`
  constant; `AiConfig::resolve_api_key()`." The split must keep all of these
  reachable at their current paths so the table stays accurate. (Do **not** edit
  CLAUDE.md in this phase — see Out of scope.)

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes
   (`git status` clean; `cargo fmt --all -- --check` clean — the tree is
   fmt-clean at the start of this phase, so any fmt dirt you see at the end is
   yours).
5. This is the same kind of mechanical file-split as phase-04 through phase-08.
   Follow the same discipline: **move** code verbatim, do not rewrite it;
   preserve item order within each destination file; **preserve every comment,
   doc-comment, and `// ----` / `// ──` banner line exactly**; re-export from
   `mod.rs` so external callers are untouched.
6. **The `include_str!` path adjustment is the load-bearing edit of this phase.**
   `config.rs` lives at `src/config.rs`; its 14 `include_str!` macros use paths
   relative to `src/`, i.e. `include_str!("../assets/…")`. After the split, all
   14 land in `src/config/seeds.rs`, which is one directory deeper, so every path
   must become `include_str!("../../assets/…")`. This is **not** an
   "improvement" — it is required for the moved code to compile. It is the one
   place where moved text must change. See the Spec for the exact 14 lines.
7. **Comment fidelity is part of "verbatim."** Phase-07 bounced because the
   executor silently dropped comments during a move. `config.rs` has **12
   `// ----…----` banner lines** (six banner *pairs*, each a heading sandwiched
   between two rule lines) outside the test module, and **8 `// ── … ──` section
   bars** inside `#[cfg(test)] mod tests`. Every one must survive the move to its
   destination file, character-identical. There are **no `TODO`/`FIXME`/`XXX`**
   comments in this file — do not introduce any.

## Current state

`src/config.rs` (1631 lines) is one flat file: config types, their `Default`
impls and `default_*` free functions, the `impl Config` / `impl ModelEntry` /
`impl LimitsConfig` / `impl Pricing` blocks, FHS path-helper functions, the
seeding functions, the embedded asset constants, and a 537-line
`#[cfg(test)] mod tests` (40 `#[test]` fns).

Top-of-file imports (old lines 1–3), verbatim — only three:

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
```

Top-level structure, grouped by destination file (read the file to confirm —
line numbers are a guide, not a contract):

| Old lines | Item | Dest |
|---|---|---|
| 7–54 | `Config` struct + `impl Default for Config` | `types.rs` |
| 56–88 | `SessionsConfig` (+ 2 default fns + `Default`) | `types.rs` |
| 90–106 | `DigestConfig` | `types.rs` |
| 108–153 | `ApprovalsConfig` (+ `Default`) | `types.rs` |
| 155–183 | `DaemonConfig` (+ `default_tmux_session` + `Default`) | `types.rs` |
| 185–216 | `GhostDaemonConfig` (+ 2 default fns + `Default`) | `types.rs` |
| 218–359 | `LimitsConfig` (+ 3 default fns + `Default` + `impl LimitsConfig`) | `types.rs` |
| 361–369 | `NotificationsConfig` | `types.rs` |
| 371–429 | `WebhookConfig` (+ 5 default fns + `Default`) | `types.rs` |
| 431–451 | `ContextConfig` (+ `Default` + `default_environment`) | `types.rs` |
| 453–462 | `MaskingConfig` | `types.rs` |
| 464–639 | `ModelEntry` (+ 2 default fns + `Default` + `impl ModelEntry`) | `types.rs` |
| 641–643 | banner `// --- Pricing schema ---` | `types.rs` |
| 645–687 | `PricingSource` enum, `Pricing` struct + `impl Pricing` | `types.rs` |
| 689–693 | `default_models()` | `types.rs` |
| 695–715 | `AiConfig` (+ `default_prompt` + `Default`) | `types.rs` |
| 717–719 | banner `// --- Prompt definitions ---` | `types.rs` |
| 721–737 | `PromptDef` + `impl PromptDef` | `types.rs` |
| 739–815 | 13 path-helper fns + `dirs_next()` (`config_dir` … `sessions_dir`) | `load.rs` |
| 817–888 | `impl Config` **except `ensure_dirs`** (`resolve_model`, `available_models`, `load`, `validate_pricing`, `scripts_dir`, `runbooks_dir`, `schedules_path`) | `load.rs` |
| 890–945 | `impl Config { ensure_dirs }` (the seeding orchestrator) | `seeds.rs` |
| 947–982 | `seed_knowledge_memory`, `seed_session_memory`, `seed_memory_inner`, `seed_agent` | `seeds.rs` |
| 986–1026 | `overwrite_knowledge_memories` | `seeds.rs` |
| 1030–1034 | `overwrite_sre_prompt` | `seeds.rs` |
| 1036–1053 | `load_named_prompt` | `load.rs` |
| 1055–1093 | 4 banner pairs + `SRE_PROMPT_TOML` + 12 asset consts | `seeds.rs` |
| 1095–1631 | `#[cfg(test)] mod tests` (40 `#[test]` fns, 8 `// ──` bars) | `mod.rs` |

**Note the two non-obvious placements above:**

- **`impl Config` is split across two files.** `resolve_model`,
  `available_models`, `load`, `validate_pricing`, `scripts_dir`, `runbooks_dir`,
  `schedules_path` go to `load.rs`; **only `ensure_dirs` goes to `seeds.rs`**
  (it is the seeding orchestrator — it references all 12 asset consts and the
  seed fns, which all live in `seeds.rs`). Rust allows multiple inherent `impl`
  blocks for the same type in different files of the same crate; this is
  intended, not a mistake. `ensure_dirs` calls `Self::scripts_dir()` and
  `Self::runbooks_dir()` (defined in `load.rs`) — these resolve fine across impl
  blocks because they are inherent methods on the same `Config`.
- **`load_named_prompt` goes to `load.rs`, not `seeds.rs`**, even though it sits
  physically between two seeding functions in the old file. It is prompt-*loading*
  logic, not seeding. It references `SRE_PROMPT_TOML` (now in `seeds.rs`) — see
  the cross-module wiring below.

External callers — these `crate::config::*` paths **must keep resolving
unchanged** (verified by grep across `src/`):

```
Types:    Config, ModelEntry, ApprovalsConfig, LimitsConfig, SessionsConfig,
          Pricing, PricingSource
Fns:      config_dir, bin_dir, pipe_log_dir, pane_logs_dir, var_run_dir,
          sessions_dir, events_path, default_socket_path, prompts_dir,
          load_named_prompt, overwrite_knowledge_memories, overwrite_sre_prompt
Methods:  Config::ensure_dirs (called from main.rs, setup.rs, costs.rs)
```

`src/lib.rs:8` declares the module: `pub mod config;`. **This line stays
exactly as-is** — a directory module `src/config/mod.rs` satisfies `pub mod
config;` identically to the old `src/config.rs` file. No `lib.rs` edit.

Cross-module references that cross the new file boundaries (the only wiring you
must add):

- `load.rs`'s `load_named_prompt` references `SRE_PROMPT_TOML` (→ `seeds.rs`).
- `load.rs`'s `impl Config` references `Config` and `ModelEntry` (→ `types.rs`).
- `seeds.rs`'s `ensure_dirs` / `overwrite_sre_prompt` reference the path-helper
  fns and `Config` (→ `load.rs` / `types.rs`). The asset consts and seed fns it
  uses are all local to `seeds.rs`.
- The test module references many types, `load_named_prompt`, and
  `SRE_PROMPT_TOML` — resolved via `mod.rs` re-exports plus one explicit `use`
  (see Spec § 4).
- `seed_agent` references `crate::agents::agent_dir` by full path (unchanged —
  leave it).

## Spec

Delete `src/config.rs` and replace it with a `src/config/` directory of four
files: `mod.rs`, `types.rs`, `load.rs`, `seeds.rs`. Move code **verbatim** —
same item bodies, same comments, same banner lines, same order within each
destination — **except** the two required edits called out below (the
`include_str!` path adjustment, and the `SRE_PROMPT_TOML` visibility bump).

### The `include_str!` path adjustment (do this, it is required)

All 14 `include_str!` calls land in `src/config/seeds.rs` (13 in the asset-const
block; one inside `ensure_dirs`). Each path gains exactly one `../` so it still
resolves from the deeper directory. The complete list — old text → new text:

```
include_str!("../assets/etc/config.toml")                              → include_str!("../../assets/etc/config.toml")
include_str!("../assets/prompts/sre.toml")                             → include_str!("../../assets/prompts/sre.toml")
include_str!("../assets/memory/knowledge/webhook-setup.md")            → include_str!("../../assets/memory/knowledge/webhook-setup.md")
include_str!("../assets/memory/knowledge/runbook-format.md")           → include_str!("../../assets/memory/knowledge/runbook-format.md")
include_str!("../assets/memory/knowledge/runbook-ghost-template.md")   → include_str!("../../assets/memory/knowledge/runbook-ghost-template.md")
include_str!("../assets/memory/knowledge/ghost-shell-guide.md")        → include_str!("../../assets/memory/knowledge/ghost-shell-guide.md")
include_str!("../assets/memory/knowledge/scheduling-guide.md")         → include_str!("../../assets/memory/knowledge/scheduling-guide.md")
include_str!("../assets/memory/knowledge/scripts-and-sudoers.md")      → include_str!("../../assets/memory/knowledge/scripts-and-sudoers.md")
include_str!("../assets/memory/knowledge/agent-runtime-layout.md")     → include_str!("../../assets/memory/knowledge/agent-runtime-layout.md")
include_str!("../assets/memory/session/pane-referencing-convention.md")→ include_str!("../../assets/memory/session/pane-referencing-convention.md")
include_str!("../assets/memory/session/unicode-decoration-pref.md")    → include_str!("../../assets/memory/session/unicode-decoration-pref.md")
include_str!("../assets/agents/architect/config.toml")                 → include_str!("../../assets/agents/architect/config.toml")
include_str!("../assets/agents/researcher/config.toml")                → include_str!("../../assets/agents/researcher/config.toml")
include_str!("../assets/agents/sysadmin/config.toml")                  → include_str!("../../assets/agents/sysadmin/config.toml")
```

The macro **bodies and const names are otherwise unchanged** — only the leading
`../` → `../../` on the path string. After building, `grep -rn 'include_str!' src/config/`
must show all 14 with `../../assets/`, and zero with `../assets/`.

### Import strategy (read this before writing any file)

The old file uses only three `use` lines. Rather than hand-derive each new
file's exact import set, use this deterministic procedure for **each** new `.rs`
file (the same one phase-08 used successfully):

1. Start the file with the **three-line import header** copied verbatim
   (`use anyhow::{Context, Result}; use serde::{Deserialize, Serialize};
   use std::path::PathBuf;`), plus the `use super::…` line(s) named in each
   step below for cross-file references.
2. Build (`cargo build`) and lint
   (`cargo clippy --all-targets --all-features -- -D warnings`).
3. The compiler/clippy will name **each** unused import (`unused import: \`…\``).
   **Remove exactly the imports it names — add nothing, guess nothing.** If it
   names an *unresolved* path instead, add the `use super::…` the message points
   to. Trust the compiler over any sketch.

The per-file sketches below are a **starting guide**, not a contract — the
compiler is the authority.

### 1. Create `src/config/types.rs`

Move, in original order, every item in the "types.rs" rows of the Current-state
table (old 7–737): all config structs/enums, their `default_*` free functions,
their `Default` impls, `impl LimitsConfig`, `impl ModelEntry`, `impl Pricing`,
`PricingSource`, `Pricing`, `default_models()`, `AiConfig`, `PromptDef` +
`impl PromptDef`, **including both banner pairs**
(`// --- Pricing schema ---` and `// --- Prompt definitions ---`).

No cross-module references — this file is self-contained (it defines the types
others depend on). Import sketch (then prune): `use serde::{Deserialize,
Serialize};` is the certain survivor; `Context`/`Result`/`PathBuf` are likely
unused here (the compiler will say). `log::warn!` (in `LimitsConfig::validate`)
uses a full path — no import needed.

### 2. Create `src/config/load.rs`

Move, in original order:
- the 13 path-helper fns + `dirs_next()` (old 739–815),
- the `impl Config` block **minus `ensure_dirs`** (old 817–888): `resolve_model`,
  `available_models`, `load`, `validate_pricing`, `scripts_dir`, `runbooks_dir`,
  `schedules_path`,
- `load_named_prompt` (old 1036–1053).

This file references `Config`/`ModelEntry`/`PromptDef` (→ `types.rs`) and
`SRE_PROMPT_TOML` (→ `seeds.rs`). Add:

```rust
use super::types::*;
use super::seeds::SRE_PROMPT_TOML;
```

Then prune `super::types::*` to the specific names if clippy prefers (it will
name unused glob members only as a whole — keep the glob if any member is used;
the compiler decides). Import sketch for the std header (then prune):
`use anyhow::{Context, Result};` (for `load`) and `use std::path::PathBuf;` (for
the path helpers) are the likely survivors.

### 3. Create `src/config/seeds.rs`

Move, in original order:
- `impl Config { ensure_dirs }` (old 890–945) — a fresh `impl Config { … }`
  block containing only `ensure_dirs`, moved verbatim **except** the one
  `include_str!` path inside it (the config.toml seed),
- `seed_knowledge_memory`, `seed_session_memory`, `seed_memory_inner`,
  `seed_agent` (old 947–982),
- `overwrite_knowledge_memories` (old 986–1026),
- `overwrite_sre_prompt` (old 1030–1034),
- the **four banner pairs** + `SRE_PROMPT_TOML` + the 12 asset consts
  (old 1055–1093), with all 13 `include_str!` paths adjusted per §
  "The `include_str!` path adjustment."

**Visibility change:** `SRE_PROMPT_TOML` is currently a private
`const SRE_PROMPT_TOML: &str = …`. Change it to
`pub(crate) const SRE_PROMPT_TOML: &str = …` so `load.rs` and the test module
can reference it. The other 12 asset consts stay private (only `ensure_dirs` /
`overwrite_*` use them, all in this file). Do not change any other visibility.

`impl Config` here requires `Config` in scope, and `ensure_dirs` /
`overwrite_sre_prompt` call the path-helper fns. Add:

```rust
use super::types::Config;
use super::load::*;
```

Then prune per the procedure. `use anyhow::{Context, Result};` is a likely std
survivor (the seed fns return `Result` and use `.with_context`).

### 4. Create `src/config/mod.rs`

The module root. It declares the three submodules, re-exports their public
surface so external `crate::config::*` paths resolve unchanged, and **holds the
`#[cfg(test)] mod tests` block** (moved verbatim from old 1095–1631, including
its 8 `// ── … ──` bars and all 40 `#[test]` fns):

```rust
//! Configuration: `~/.daemoneye/etc/config.toml` parsing, FHS path helpers,
//! and first-run asset seeding. Split across submodules in phase-09; the
//! public surface is re-exported here.

mod load;
mod seeds;
mod types;

pub use load::*;
pub use seeds::*;
pub use types::*;
```

Then the test module below the re-exports. The original test module begins:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    …
}
```

Keep `use super::*;` (it resolves the re-exported public names — `Config`,
`ModelEntry`, `ApprovalsConfig`, `LimitsConfig`, `DigestConfig`, `PromptDef`,
`PricingSource`, `Pricing`, `load_named_prompt`, and the `Config` methods). The
test `builtin_sre_prompt_parses` references `SRE_PROMPT_TOML`, which `pub use
seeds::*` does **not** re-export (it is `pub(crate)`, not `pub`). Add one line
inside the test module, right after `use super::*;`:

```rust
    use super::seeds::SRE_PROMPT_TOML;
```

This is `#[cfg(test)]`-gated, so it raises no unused-import warning in normal
builds. No other test edits — do not rename, reorder, split, add, or remove any
test.

Notes:
- `pub use seeds::*;` re-exports `overwrite_knowledge_memories`,
  `overwrite_sre_prompt`, `seed_agent` (all `pub`), keeping their
  `crate::config::*` paths alive. `ensure_dirs` is a `pub` method on `Config`, so
  it resolves via the re-exported `Config` regardless of which file its impl
  block sits in.
- Do **not** add `pub use` for individual items beyond the three globs; the
  globs cover the full external surface (verified by grep). Adding redundant
  explicit re-exports risks `ambiguous_glob_reexports` or unused-import lints.

### 5. Delete the old file

Remove `src/config.rs`. `src/lib.rs:8` (`pub mod config;`) is unchanged — it now
resolves to the directory module.

### 6. Format only the new files

Do **not** run `cargo fmt --all` — phase-07 bounced partly because it
reformatted unrelated files into the commit. Format **only** the four new files:

```sh
rustfmt src/config/mod.rs src/config/types.rs src/config/load.rs src/config/seeds.rs
```

Then confirm the whole tree is still fmt-clean (it was clean at phase start, so
it must be clean now): `cargo fmt --all -- --check` → no output. If `--check`
reports a file **you did not create or move**, you have a collateral-fmt
problem — revert that file, do not commit it.

### 7. Commit

One `refactor:` commit. The diff should touch **only**: the deleted
`src/config.rs`, the four new `src/config/*.rs` files, and the two phase-doc
status updates (this file + the README row). `git diff --stat` must show **no**
changes to `src/lib.rs` or any other source file.

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all -- --check` passes (whole tree clean).
- [ ] `cargo test` passes — the **same** test count as before this phase (no
      tests added, removed, or renamed; the 40 `#[test]` fns from the old `mod
      tests` all still run and pass under `config::tests`).
- [ ] `src/config.rs` no longer exists; `src/config/` contains exactly
      `mod.rs`, `types.rs`, `load.rs`, `seeds.rs`.
- [ ] `src/lib.rs` still reads `pub mod config;` (line ~8), unchanged. `git diff
      --stat` shows no change to `src/lib.rs`.
- [ ] **`include_str!` fidelity:** `grep -rn 'include_str!' src/config/` shows
      all 14 calls with `../../assets/`, and **zero** with `../assets/`. The
      embedded assets actually load: see End-to-end verification.
- [ ] **Comment fidelity:** all 12 `// ----` banner lines (six pairs) survive in
      `types.rs` (Pricing schema, Prompt definitions) and `seeds.rs` (SRE prompt,
      knowledge memories, session memories, named agents), and all 8 `// ── … ──`
      bars survive in `mod.rs`'s test block, character-identical. Spot-check:
      `grep -rcn '// ----' src/config/` across the four files sums to 12;
      `grep -cn '// ──' src/config/mod.rs` is 8.
- [ ] **Line-fidelity check (sorted-multiset):** the concatenated non-blank,
      trimmed lines of the four new files, minus the new glue (the module
      doc-comment + `mod`/`pub use` headers in `mod.rs`, the `use super::…` lines,
      the `#[cfg(test)]` test-only `use super::seeds::SRE_PROMPT_TOML;` line) and
      after normalizing the two required text edits (`../assets/` → `../../assets/`
      on the 14 `include_str!` paths; `const SRE_PROMPT_TOML` →
      `pub(crate) const SRE_PROMPT_TOML`), equal the non-blank trimmed lines of
      the old `src/config.rs`. Spot-check by diffing one representative moved item
      (e.g. `impl ModelEntry`'s `context_window`) old-vs-new — its body must be
      character-identical.

## Test plan

No new tests. This phase **moves** the existing `#[cfg(test)] mod tests` verbatim
into `mod.rs`. The acceptance bar is that all 40 pre-existing tests still compile
and pass after the move. Named regression anchors that must still pass (all now
under `config::tests`):

- `default_config_has_default_model`, `parse_models_section`,
  `available_models_returns_sorted_keys`,
  `resolve_model_unknown_name_falls_back_to_default` — prove the `impl Config`
  resolve/available logic moved intact into `load.rs` and is reachable.
- `builtin_sre_prompt_parses`, `load_sre_prompt_falls_back_to_builtin`,
  `load_unknown_prompt_returns_minimal` — prove `SRE_PROMPT_TOML` (now `pub(crate)`
  in `seeds.rs`) and `load_named_prompt` (now in `load.rs`) still resolve across
  the new module boundary, and the embedded SRE prompt const is intact.
- `default_limits_match_current_hardcoded_constants`, `cap_u32_sentinel`,
  `per_tool_cap_uses_override_over_global`,
  `validate_approval_gated_per_tool_entry_does_not_panic` — prove `LimitsConfig`
  and its `impl` moved intact into `types.rs`.
- `local_provider_pricing_is_zero`,
  `model_entry_with_explicit_pricing_overrides_defaults` — prove `Pricing` /
  `PricingSource` / `impl ModelEntry::pricing` moved intact.

## End-to-end verification

The phase ships a runtime-loadable artifact: the daemon's first-run seeding
(`Config::ensure_dirs`) writes `config.toml`, the SRE prompt, the seeded
memories, and the example agents from the 14 `include_str!`-embedded assets. A
wrong `include_str!` path is a **compile-time** failure (the build cannot find
the file), so a successful `cargo build` already proves all 14 paths resolve.
Additionally confirm the embedded content is non-empty at runtime by running the
existing test that parses the embedded SRE prompt, and quote its result:

```sh
cargo test --lib config::tests::builtin_sre_prompt_parses -- --nocapture
```

Quote the actual `test result: ok.` line in the completion Update Log. (This
test deserializes `SRE_PROMPT_TOML` and asserts a non-empty system prompt — it
fails if the const moved to a wrong path or lost its `include_str!` target.)

## Authorizations

- [ ] May touch `docs/architecture.md`: **No.**
- [ ] May add dependencies: **No.**
- [ ] May edit `CLAUDE.md`: **No.** (Its file-table lists `src/config.rs`;
      updating that entry to the `config/` submodule is a follow-up doc task, not
      part of this mechanical phase — leave CLAUDE.md alone.)
- [ ] May edit `src/lib.rs`: **No.** The `pub mod config;` declaration resolves
      to the directory module unchanged.

Otherwise: None.

## Out of scope

- **Do not** move `ensure_dirs` anywhere other than `seeds.rs`, and do not move
  the other `impl Config` methods out of `load.rs`. The `impl Config` split
  (load.rs = 7 methods; seeds.rs = `ensure_dirs`) is exactly as specified.
- **Do not** "consolidate" or "tidy" the `default_*` free functions, merge
  duplicate-looking ones (e.g. the several that return `true`/strings), or
  collapse the `impl Default` blocks into `#[derive(Default)]`. Move them
  verbatim.
- **Do not** change any visibility except the single specified bump
  (`SRE_PROMPT_TOML` → `pub(crate)`). Leave the 12 other asset consts private;
  leave every fn/struct/method visibility as-is.
- **Do not** alter any `include_str!` target *beyond* the `../` → `../../`
  prefix adjustment — the asset paths, const names, and types are unchanged.
- **Do not** rename, add, remove, merge, re-signature, reorder, or split any
  struct, fn, method, const, or test. The only permitted edits are the new
  module/import glue, the `SRE_PROMPT_TOML` visibility bump, the 14
  `include_str!` path adjustments, and the one `#[cfg(test)]`
  `use super::seeds::SRE_PROMPT_TOML;` line.
- **Do not** edit `src/lib.rs`, `CLAUDE.md`, `docs/architecture.md`, or any
  caller. If the re-exports in `mod.rs` are correct, no caller needs to change.
- **Do not** run `cargo fmt --all` (it sweeps unrelated files into the commit —
  see step 6). Format only the four new files.
- **Do not** "improve" any code you move — no clippy-pleasing rewrites, no
  reordering of match arms or struct fields, no comment edits, no dropping of
  banner lines.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-26 20:43 (started)

**Executor:** rexyMCP executor

Split `src/config.rs` (1631 lines) into `src/config/` submodule: `types.rs`, `load.rs`, `seeds.rs`, `mod.rs`. Adjusted 14 `include_str!` paths from `../assets/` to `../../assets/`. Bumped `SRE_PROMPT_TOML` to `pub(crate)`. Moved test module verbatim into `mod.rs` with one additional `use super::seeds::SRE_PROMPT_TOML;` line.

### Update — 2026-06-26 20:44 (complete)

**Verification commands:**
- `cargo build` — succeeded with zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — passed clean
- `cargo fmt --all -- --check` — whole tree clean
- `cargo test` — all 40 `config::tests` tests pass (27 total integration tests pass, 2 ignored)
- `cargo test --lib config::tests::builtin_sre_prompt_parses -- --nocapture` — `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 772 filtered out`

**End-to-end verification:** `builtin_sre_prompt_parses` confirmed `SRE_PROMPT_TOML` (now in `seeds.rs`) deserializes to a valid `PromptDef` with a non-empty system prompt. All 14 `include_str!` paths resolve with `../../assets/` prefix (zero with `../assets/`).

**Comment fidelity:** 12 `// ----` banner lines across `types.rs` (4) and `seeds.rs` (8); 8 `// ──` bars in `mod.rs` test module.

**`include_str!` grep proof:** `grep -rn 'include_str!' src/config/` shows 14 lines, all with `../../assets/`; `grep -rn 'include_str!("../assets/' src/config/` returns 0.

**Files changed:**
- Deleted: `src/config.rs` (1631 lines)
- Created: `src/config/types.rs`, `src/config/load.rs`, `src/config/seeds.rs`, `src/config/mod.rs`
- Updated: phase-09-split-config.md (status + Update Log), M2 README (phase table)

**Notes for review:** None — pure mechanical split, no behavior changes.
