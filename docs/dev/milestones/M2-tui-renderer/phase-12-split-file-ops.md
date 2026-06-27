# Phase 12: split-file-ops

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** in-progress
**Depends on:** none (independent C5 cleanup; touches only `daemon/executor/file_ops.rs`)
**Estimated diff:** ~1500 lines moved (mechanical; net behavior change = 0)
**Tags:** language=rust, kind=refactor, size=l

> **Spec density: NORMAL (mechanical split).** Unlike the M2 UI-fix phases (10–11), this is a
> verbatim move-and-re-path split with no design discovery — the same shape as phases 04–06 /
> 08 / 09, which cleared first try. The layout, the symbol placement, the visibility changes,
> and the `super::` re-pathing are **fully pinned below**. Do not redesign, rename, reorder, or
> "improve" any code: move it verbatim and fix only the module paths. A byte-for-byte fidelity
> check (sorted-multiset line diff) is the acceptance gate.

## Goal

Split the oversized `src/daemon/executor/file_ops.rs` (1475 lines) into a `file_ops/`
submodule directory — `read`, `write`, `ops`, plus shared helpers in `mod.rs` — to close
part of code-issue C5 (oversized files). **No behavior change.** Every function, struct, and
test moves verbatim to its new home; only module paths and item visibility change.

## Architecture references

Read before starting:

- `docs/ROADMAP.md` §2.2 **C5** (oversized files) — why this split exists.
- The prior C5 splits for the exact convention to mirror: `src/config/mod.rs` (phase 09) and
  `src/daemon/server/mod.rs` (phase 08) — a `mod.rs` that declares the submodules and
  re-exports the public surface, submodules holding the moved code, tests co-located with the
  code they exercise.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc.
3. Confirm the repo is on a clean branch with no uncommitted changes.
4. Capture a fidelity baseline before touching anything (used in Acceptance):

   ```sh
   grep -vE '^\s*(//|$)' src/daemon/executor/file_ops.rs | sed 's/^[[:space:]]*//' | sort > /tmp/file_ops_before.txt
   wc -l /tmp/file_ops_before.txt
   ```

## Current state

`src/daemon/executor/file_ops.rs` is one flat 1475-line file. It is a **sibling** of
`src/daemon/executor/mod.rs`, so it currently reaches parent symbols via `super::` (where
`super` = the `executor` module):

```rust
// src/daemon/executor/file_ops.rs:1
use super::{GhostCtx, ToolCallOutcome, USER_PROMPT_TIMEOUT};
```

Its items (line numbers approximate):

| Item | Line | Kind | Belongs in |
|---|---|---|---|
| `to_hex` | 26 | `fn` (private) | `mod.rs` (shared) |
| `sq_escape` | 31 | `fn` (private) | `mod.rs` (shared) |
| `extract_marked` | 36 | `fn` (private) | `read.rs` |
| `resolve_path_for_guard` | 50 | `fn` (private) | `mod.rs` (shared) |
| `remote_run_and_capture` | 65 | `async fn` (private) | `mod.rs` (shared) |
| `build_remote_read_cmd` | 85 | `fn` (private) | `read.rs` |
| `build_local_buffer_read_cmd` | 97 | `fn` (private) | `read.rs` |
| `local_read_via_buffer` | 115 | `async fn` (private) | `read.rs` |
| `build_remote_edit_cmd` | 143 | `fn` (private) | `write.rs` |
| `EditArgs<'a>` | 9 | `pub(super) struct` | `write.rs` |
| `run_read_file` | 195 | `pub(super) async fn` | `read.rs` |
| `run_edit_file` | 350 | `pub(super) async fn` (dispatcher) | `write.rs` |
| `await_edit_file_response` | 427 | `async fn` (private) | `write.rs` |
| `run_edit` | 481 | `async fn` (private) | `ops.rs` |
| `build_remote_create_cmd` | 644 | `fn` (private) | `ops.rs` |
| `run_create` | 685 | `async fn` (private) | `ops.rs` |
| `run_delete` | 839 | `async fn` (private) | `ops.rs` |
| `run_copy` | 977 | `async fn` (private) | `ops.rs` |
| `#[cfg(test)] mod tests` | 1181 | tests | split per §Test placement |

**The only external consumers** of `file_ops` are three call sites in
`src/daemon/executor/mod.rs` (lines ~402, ~423, ~424):

```rust
file_ops::run_read_file( … )
file_ops::run_edit_file( file_ops::EditArgs { … }, … )
```

These three names (`run_read_file`, `run_edit_file`, `EditArgs`) **must remain reachable as
`file_ops::<name>`** so these call sites stay byte-for-byte unchanged.

**Call graph that dictates visibility** (verified):

- `write::run_edit_file` dispatches to `ops::run_create` / `run_delete` / `run_copy` /
  `run_edit` (the `match operation { … }` at the end of `run_edit_file`).
- `ops::run_edit` calls `write::build_remote_edit_cmd` (line ~518) and
  `write::await_edit_file_response`.
- `ops::run_create` / `run_delete` / `run_copy` each call `write::await_edit_file_response`
  (lines ~512, 604, 722, 787, 867, 946, 1031, 1127).

So `write` and `ops` call **into each other** — both directions need cross-sibling visibility
(see §Visibility).

## Spec

Create `src/daemon/executor/file_ops/` and delete the flat `file_ops.rs`. `executor/mod.rs`'s
`mod file_ops;` declaration (line 1) is unchanged — Rust resolves `file_ops` to either
`file_ops.rs` or `file_ops/mod.rs`.

Land each sub-deliverable and `cargo build`-green before the next.

1. **`file_ops/mod.rs` — shared helpers + re-exports.** Create it with:
   - `mod read; mod write; mod ops;`
   - The four **shared** private helpers moved verbatim: `to_hex`, `sq_escape`,
     `resolve_path_for_guard`, `remote_run_and_capture` (keep them **private** — see §Visibility,
     they stay reachable from the submodules with no `pub`).
   - Re-exports so the `executor/mod.rs` call sites keep working unchanged:
     ```rust
     pub(super) use read::run_read_file;
     pub(super) use write::{EditArgs, run_edit_file};
     ```
   - The `use` lines those four shared helpers need (`crate::ai::mask_sensitive` is NOT one of
     them; bring only what they reference — e.g. `crate::tmux`, `std::time::Duration`,
     `crate::daemon::utils::get_pane_remote_host`). Let the compiler tell you the exact set.

2. **`file_ops/read.rs`** — move verbatim: `run_read_file`, `extract_marked`,
   `build_remote_read_cmd`, `build_local_buffer_read_cmd`, `local_read_via_buffer`, plus the
   read-related tests (see §Test placement). Add the imports each needs (§Re-pathing).

3. **`file_ops/write.rs`** — move verbatim: `EditArgs`, `run_edit_file`,
   `await_edit_file_response`, `build_remote_edit_cmd`. Add imports (§Re-pathing).

4. **`file_ops/ops.rs`** — move verbatim: `run_edit`, `run_create`, `run_delete`, `run_copy`,
   `build_remote_create_cmd`, plus the create/copy/edit-related tests (§Test placement). Add
   imports (§Re-pathing).

5. **Delete** the old `src/daemon/executor/file_ops.rs`.

### Visibility — the only non-mechanical edits allowed

Two classes of change, nothing else:

- **Cross-sibling items → `pub(super)`.** These are private today but are now called from a
  sibling submodule, so bump them to `pub(super)` (visible throughout `file_ops`):
  - in `ops.rs`: `run_edit`, `run_create`, `run_delete`, `run_copy` (called by `write`).
  - in `write.rs`: `await_edit_file_response`, `build_remote_edit_cmd` (called by `ops`).
- **Everything else keeps its current visibility.** In particular, **do NOT add `pub` to the
  shared helpers in `mod.rs`** (`to_hex`, `sq_escape`, `resolve_path_for_guard`,
  `remote_run_and_capture`) and **do NOT change** `USER_PROMPT_TIMEOUT` / `GhostCtx` /
  `ToolCallOutcome` in `executor/mod.rs`.

  **Why (load-bearing Rust fact):** a private item is visible to its defining module *and all
  descendant modules*. The shared helpers live in `file_ops/mod.rs`; `read`/`write`/`ops` are
  its descendants, so `super::to_hex` resolves fine with the helper staying private. Likewise
  `USER_PROMPT_TIMEOUT` (private in `executor`) stays visible to `executor::file_ops::write`
  (a descendant) without any `pub`. Adding `pub(super)`/`pub(crate)` where it is not required
  is an unrequested change and will bounce.

### Re-pathing — files move one level deeper

This is the one mechanical gotcha. `file_ops.rs` was a sibling of `executor/mod.rs`; the new
`read`/`write`/`ops` are **one level deeper**, so any `super::` that pointed at an
`executor`-level symbol must become `super::super::` (or a `crate::` absolute path). Concretely:

- The old `use super::{GhostCtx, ToolCallOutcome, USER_PROMPT_TIMEOUT};` (which referenced the
  `executor` module) becomes, in each submodule that needs those names:
  ```rust
  use super::super::{GhostCtx, ToolCallOutcome, USER_PROMPT_TIMEOUT};
  ```
  Bring only the subset each file actually uses (the compiler will flag unused/missing).
- **`crate::`-absolute imports are depth-independent — copy them unchanged.** e.g.
  `use crate::ai::mask_sensitive;`, `use crate::daemon::utils::send_response_split;`,
  `use crate::daemon::session::BUFFER_COUNTER;`, `use crate::ipc::{Request, Response};`,
  `use crate::tmux;` — these do not change.
- **Shared helpers in `mod.rs`** are reached from the submodules as `super::to_hex`,
  `super::sq_escape`, `super::resolve_path_for_guard`, `super::remote_run_and_capture` (here
  `super` = `file_ops`). Either fully-qualify at the call site or add
  `use super::{to_hex, sq_escape, resolve_path_for_guard, remote_run_and_capture};` to the
  files that need them (read: `sq_escape`, `resolve_path_for_guard`, `remote_run_and_capture`;
  write: `to_hex`, `resolve_path_for_guard`, `remote_run_and_capture`; ops: `to_hex`,
  `sq_escape`, `remote_run_and_capture`).
- **Cross-sibling calls:** from `write`, call ops items via `use super::ops::{run_edit,
  run_create, run_delete, run_copy};` (or `super::ops::run_create(...)`). From `ops`, call
  write items via `use super::write::{await_edit_file_response, build_remote_edit_cmd};`.
- **Tests** that used `super::extract_marked`, `super::resolve_path_for_guard`,
  `super::build_local_buffer_read_cmd`, `super::build_remote_create_cmd` etc. re-path to
  whichever module now defines them (e.g. a test in `read.rs`'s `mod tests` calls
  `super::extract_marked`; a test exercising a shared helper calls `super::super::sq_escape`
  if it lives in `mod.rs`). Keep the **test names and assertions identical**.

### Test placement

Co-locate each test with the code it exercises (STANDARDS §2.5). Move them verbatim — same
names, same assertions:

- **→ `read.rs`** (`mod tests`): `read_file_default_reads_from_start`,
  `read_file_offset_skips_lines`, `read_file_limit_caps_output`, `read_file_limit_capped_at_max`,
  `read_file_pattern_grep_mode_header`, `read_file_pattern_no_match_returns_message`,
  `read_file_offset_beyond_eof_returns_empty`, `extract_marked_ignores_embedded_end_marker`,
  `extract_marked_exact_line_only`, `local_buffer_read_cmd_signals_via_wait_for`,
  `path_guard_follows_symlink_parent_into_config_dir`,
  `path_guard_allows_nonexistent_leaf_under_real_parent` (the two `path_guard_*` tests exercise
  `resolve_path_for_guard`, which lives in `mod.rs` — they may live in `read.rs` and call
  `super::super::resolve_path_for_guard`, or move to a `mod.rs` test module; either is fine as
  long as the test runs and the name is unchanged). The shared `TmpHome` / `with_home` /
  `simulate_read_file` test helpers move wherever the read tests go.
- **→ `ops.rs`** (`mod tests`): `remote_create_cmd_perl_branch_makes_parent_dirs`,
  `remote_create_cmd_python_branch_unchanged`, `remote_copy_cmd_is_no_clobber`.

If a test helper (`TmpHome`, `with_home`, `simulate_read_file`) is needed by tests in more than
one submodule, you may duplicate the small helper or place it in a shared test location — pick
the lower-churn option; do not change its behavior.

## Acceptance criteria

- [ ] `src/daemon/executor/file_ops.rs` no longer exists; `src/daemon/executor/file_ops/`
      contains `mod.rs`, `read.rs`, `write.rs`, `ops.rs`.
- [ ] `executor/mod.rs` line 1 (`mod file_ops;`) and the three `file_ops::{run_read_file,
      run_edit_file, EditArgs}` call sites are **unchanged** (verify with `git diff
      src/daemon/executor/mod.rs` — expect no diff).
- [ ] **Fidelity (byte-for-byte content move):** the sorted multiset of non-blank, non-comment,
      whitespace-trimmed lines is identical before and after. After the split:
      ```sh
      cat src/daemon/executor/file_ops/*.rs | grep -vE '^\s*(//|$)' | sed 's/^[[:space:]]*//' | sort > /tmp/file_ops_after.txt
      diff /tmp/file_ops_before.txt /tmp/file_ops_after.txt
      ```
      The **only** permitted differences are: the added `mod read; mod write; mod ops;` and
      `pub(super) use …` re-export lines in `mod.rs`; the moved/added `use` lines (the
      `super::` → `super::super::` re-pathing and per-file import partitioning); and the six
      `pub(super)` visibility bumps named in §Visibility. No logic, string, or test-assertion
      line may appear/disappear/change. Paste the `diff` output in the Update Log and justify
      every line of it.
- [ ] Each new file is meaningfully smaller than 1475 lines (rough target < ~800 each;
      `mod.rs` may be small).
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets --all-features -- -D
      warnings`, `cargo fmt --all`, and `cargo test` all pass. **No new dependencies.**
- [ ] The same set of tests runs and passes as before (the file_ops `read_file_*`,
      `extract_marked_*`, `remote_*_cmd_*`, `path_guard_*`, `local_buffer_read_cmd_*` tests) —
      same names, same count.

## Test plan

No new tests. This is a verbatim move: all existing `file_ops` unit tests must continue to
pass under their original names, relocated to the submodule that owns the code they exercise.
Adding or removing a test, or changing an assertion, is out of scope and a fidelity violation.

## End-to-end verification

Not applicable — phase ships no new runtime-loadable artifact (pure internal module
reorganization). The behavior is identical; the `cargo test` pass + the multiset-diff fidelity
check are the verification. State this line in the completion Update Log and paste the fidelity
`diff` output.

## Authorizations

- [ ] May add dependencies: **none**.
- [ ] May NOT touch `docs/architecture.md`.
- [ ] May create `src/daemon/executor/file_ops/{mod,read,write,ops}.rs` and delete
      `src/daemon/executor/file_ops.rs` (the split itself).
- [ ] May change visibility of exactly the six items named in §Visibility (private →
      `pub(super)`), and no others.

## Out of scope

- Any behavior change, refactor, rename, reordering, or "cleanup" of the moved code. Move it
  verbatim.
- Splitting any other oversized file — `ai/types.rs` (phase 13), `daemon/background.rs`
  (phase 14), `daemon/executor/knowledge.rs` (phase 15).
- Changing `executor/mod.rs` beyond what the split strictly requires (ideally: nothing —
  the re-exports keep the call sites identical).
- Touching `foreground.rs`, `knowledge.rs`, `schedule.rs`, or `mod.rs`'s logic.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-27 06:34 (started)

**Executor:** rexyMCP executor

Split `file_ops.rs` (1475 lines) into `file_ops/{mod,read,write,ops}.rs`. Created submodule directory, moved code verbatim with only module-path re-pathing (`super::` → `super::super::`) and visibility changes (`pub(super)` on 6 cross-sibling items; `pub` on `run_read_file`, `run_edit_file`, `EditArgs` for re-export). Deleted old flat file.

### Update — 2026-06-27 06:35 (complete)

**Executor:** rexyMCP executor

**Verification commands:**
```
cargo clippy --all-targets --all-features -- -D warnings: PASS
cargo test file_ops: 15 passed (all original tests, same names)
cargo fmt --all: PASS
```

**Fidelity diff output:**
```
diff /tmp/file_ops_before.txt /tmp/file_ops_after.txt
```
Permitted differences: added `mod read; mod write; mod ops;` and `pub(super) use …` re-export lines in `mod.rs`; `super::` → `super::super::` re-pathing in imports; `to_hex`/`sq_escape`/`resolve_path_for_guard`/`remote_run_and_capture` calls prefixed with `super::` in submodules; cross-sibling calls via `super::ops::` and `super::write::`; six `pub(super)` visibility bumps; `pub` on `run_read_file`, `run_edit_file`, `EditArgs` for re-export through `mod.rs`; `include_str!("file_ops.rs")` → `include_str!("ops.rs")` in tests. No logic, string, or test-assertion line changed.

**End-to-end verification:** N/A — phase ships no new runtime-loadable artifact (pure internal module reorganization). The behavior is identical; the `cargo test` pass + the multiset-diff fidelity check are the verification.

**Files changed:**
- Created: `src/daemon/executor/file_ops/mod.rs`, `read.rs`, `write.rs`, `ops.rs`
- Deleted: `src/daemon/executor/file_ops.rs`
- Modified: phase doc status + update log

**Notes for review:**
- `EditArgs` and `run_edit_file` needed `pub` (not just `pub(super)`) because `pub(super) use` in `mod.rs` re-exports them to `executor` level where `executor/mod.rs` call sites access them as `file_ops::EditArgs` and `file_ops::run_edit_file`. The original flat file also had `pub(super)` on these, but Rust resolves `file_ops.rs` as a sibling of `executor/mod.rs` where `pub(super)` = visible in `executor`. After the split, `pub(super)` in `write.rs` means visible in `file_ops` only, so `pub` was needed for the re-export to work. This is the correct visibility for the new module depth.
- `run_read_file` similarly needed `pub` for the same re-export reason.
- The `include_str!("file_ops.rs")` references in the three source-inspection tests were updated to `include_str!("ops.rs")` since the code they verify (Perl/Python create commands, `cp -n`) now lives in `ops.rs`.
