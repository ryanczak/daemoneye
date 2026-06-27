# Phase 14: split-background

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** done
**Depends on:** none (independent C5 cleanup; touches only `src/daemon/background.rs`)
**Estimated diff:** ~1369 lines moved (mechanical; net behavior change = 0)
**Tags:** language=rust, kind=refactor, size=l

> **Spec density: NORMAL (mechanical split).** This is a verbatim move-and-re-path split with
> no design discovery — the same shape as phases 04–06 / 08 / 09 / 12 / 13, all of which cleared
> first try. The layout, the symbol placement, the visibility bumps, the cross-sibling imports,
> and the re-exports are **fully pinned below**. Do not redesign, rename, reorder, or "improve"
> any code: move it verbatim and fix only the module paths and the visibility qualifiers named
> below. A byte-for-byte fidelity check (sorted-multiset line diff) is the acceptance gate.
>
> **This file is shaped like phase 12, not phase 13.** Two facts make it so, both verified
> against the current source — rely on them, but let the compiler confirm:
> 1. **No `super::` re-pathing of parent symbols.** `src/daemon/background.rs` reaches every
>    parent/crate symbol via **`crate::`-absolute** paths (`crate::ai::…`, `crate::daemon::…`,
>    `crate::ipc::…`, `crate::tmux`, `crate::util::…`, `crate::config::…`, `crate::manifest::…`).
>    `crate::` paths are **depth-independent — copy them unchanged.** The only `super::` in the
>    whole file is `use super::*;` in the one test module. So there is no `super::` →
>    `super::super::` rewrite (the gotcha phase 12 had). The *new* cross-module paths are the
>    cross-sibling imports of the shared `helpers` items (see §Re-pathing).
> 2. **Visibility bumps ARE required (unlike phase 13).** The shared helpers move into a
>    *sibling* `helpers.rs`, so the four helper items used from `run`/`respawn` must be bumped
>    from private to `pub(super)` — exactly the phase-12 pattern. The re-exported public surface
>    (§Spec item 1) is already `pub`, so the re-exports themselves widen nothing (no E0364).
>    The complete, closed list of visibility changes is in §Visibility — make those and no others.

## Goal

Split the oversized `src/daemon/background.rs` (1369 lines) into a `background/` submodule
directory — `helpers`, `run`, `respawn`, `gc`, plus re-exports in `mod.rs` — to close part of
code-issue C5 (oversized files). **No behavior change.** Every function, struct, const, static,
and test moves verbatim to its new home; only module paths and the named item visibilities change.

## Architecture references

Read before starting:

- `docs/ROADMAP.md` §2.2 **C5** (oversized files) — why this split exists.
- The prior C5 splits for the exact convention to mirror: `src/daemon/executor/file_ops/mod.rs`
  (phase 12 — the closest analogue, since it also needed `pub(super)` visibility bumps for
  cross-sibling helpers) and `src/ai/types/mod.rs` (phase 13 — the `mod.rs` that is *only*
  submodule declarations + `pub use` re-exports, no code of its own). This phase combines both:
  a code-free `mod.rs` (like phase 13) **and** `pub(super)` helper bumps (like phase 12).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc.
3. Confirm the repo is on a clean branch with no uncommitted changes.
4. Capture a fidelity baseline before touching anything (used in Acceptance):

   ```sh
   grep -vE '^\s*(//|$)' src/daemon/background.rs | sed 's/^[[:space:]]*//' | sort > /tmp/bg_before.txt
   wc -l /tmp/bg_before.txt   # expect 1064
   ```

## Current state

`src/daemon/background.rs` is one flat 1369-line file. It is a **sibling** of `src/daemon/mod.rs`,
declared there at line 29 (`pub mod background;`). Its top-level imports (lines 1–13) are all
`crate::`-absolute or `std::`:

```rust
use crate::ai::Message;
use crate::ai::filter::mask_sensitive;
use crate::daemon::session::{
    BgWindowInfo, SessionStore, append_session_message, bg_done_subscribe, complete_subscribe,
};
use crate::daemon::utils::{
    command_has_sudo, is_fingerprint_prompt, log_event, normalize_output, shell_escape_arg,
    sudo_auth_failed, wait_for_sudo_prompt_and_inject,
};
use crate::ipc::Response;
use crate::tmux;
use crate::util::UnpoisonExt;
use std::time::Duration;
```

There is also a mid-file `use std::sync::Mutex;` (line 237, immediately above `BG_COMMAND_MAP`).

Its items (line numbers approximate):

| Item | Lines | Kind / current visibility | Belongs in |
|---|---|---|---|
| `shell_exit_var` | 21–26 | `fn` (private) | `helpers.rs` |
| `OUTPUT_INLINE_LIMIT` | 34 | `const` (private) | `helpers.rs` |
| `trim_large_output` | 40–72 | `fn` (private) | `helpers.rs` |
| `capture_and_archive` | 85–138 | `fn` (private) | `helpers.rs` |
| `BgJobInfo<'a>` | 140–147 | `struct` (private, private fields) | `helpers.rs` |
| `notify_session` | 153–215 | `fn` (private) | `helpers.rs` |
| `use std::sync::Mutex;` | 237 | import | `helpers.rs` |
| `BG_COMMAND_MAP` | 239–240 | `pub static` | `helpers.rs` |
| `run_background_in_window` | 242–645 | `pub async fn` | `run.rs` |
| `respawn_background_in_pane` | 647–924 | `pub async fn` | `respawn.rs` |
| `OwnedJobInfo` | 927–931 | `pub struct` | `gc.rs` |
| `notify_job_completion` | 939–1002 | `pub async fn` | `gc.rs` |
| `PaneGcInfo` | 1009–1016 | `pub(crate) struct` | `gc.rs` |
| `DEAD_THRESHOLD_SECS` | 1019 | `const` (private) | `gc.rs` |
| `IDLE_THRESHOLD_SECS` | 1023 | `const` (private) | `gc.rs` |
| `IDLE_SHELLS` | 1025 | `const` (private) | `gc.rs` |
| `plan_gc_actions` | 1032–1058 | `pub(crate) fn` | `gc.rs` |
| `DAEMON_BG_PREFIXES` | 1060–… | `const` (private) | `gc.rs` |
| `gc_bg_windows` | 1077–1198 | `pub fn` | `gc.rs` |
| `#[cfg(test)] mod tests` | 1199–1369 | tests | split per §Test placement |

**External consumers** reach these types as `crate::daemon::background::<Name>` (verified — all
4 call-site files):

- `src/daemon/executor/foreground.rs:6` — `use crate::daemon::background::{respawn_background_in_pane, run_background_in_window};`
- `src/daemon/scheduled.rs:3` — `use crate::daemon::background::{OwnedJobInfo, notify_job_completion};`
- `src/daemon/hook.rs:36` — `crate::daemon::background::BG_COMMAND_MAP`
- `src/daemon/mod.rs:715` — `crate::daemon::background::gc_bg_windows(...)`

So **six names must remain reachable as `crate::daemon::background::<Name>`** after the split:
`run_background_in_window`, `respawn_background_in_pane`, `OwnedJobInfo`, `notify_job_completion`,
`BG_COMMAND_MAP`, `gc_bg_windows`. The re-exports in the new `mod.rs` (§Spec item 1) are what
preserve this. **None of these four consumer files may be edited** — the re-exports keep their
import paths byte-for-byte valid.

`PaneGcInfo` and `plan_gc_actions` are `pub(crate)` but are referenced **only inside this file**
(by `gc_bg_windows` and the gc tests). They are **not** external consumers and are **not**
re-exported; they keep their `pub(crate)` and live in `gc.rs`.

**Cross-module references that dictate the visibility bumps + cross-sibling imports** (verified):

- `run.rs` (`run_background_in_window`) calls helpers: `shell_exit_var` (line 310),
  `capture_and_archive` (464, 569), constructs `BgJobInfo` (586) and calls `notify_session`
  (583), and uses `BG_COMMAND_MAP` (294).
- `respawn.rs` (`respawn_background_in_pane`) calls the **same** helper set: `shell_exit_var`
  (680), `capture_and_archive` (770, 872), `BgJobInfo` (890), `notify_session` (887),
  `BG_COMMAND_MAP` (656).
- `gc.rs` uses **no** `helpers` items (its archive logic in `notify_job_completion` is inline and
  self-contained; nothing in gc calls `capture_and_archive` / `notify_session`).
- `OUTPUT_INLINE_LIMIT` and `trim_large_output` are used **only inside `helpers.rs`** (by
  `capture_and_archive` and the helpers tests) — they stay private, no bump.

## Spec

Create `src/daemon/background/` and delete the flat `background.rs`. `daemon/mod.rs`'s
`pub mod background;` declaration (line 29) is unchanged — Rust resolves `background` to either
`background.rs` or `background/mod.rs`.

Land each sub-deliverable and `cargo build`-green before the next.

1. **`background/mod.rs` — submodule declarations + re-exports (no code of its own).** Create it
   with exactly:

   ```rust
   mod gc;
   mod helpers;
   mod respawn;
   mod run;

   pub use gc::{OwnedJobInfo, gc_bg_windows, notify_job_completion};
   pub use helpers::BG_COMMAND_MAP;
   pub use respawn::respawn_background_in_pane;
   pub use run::run_background_in_window;
   ```

   The submodules are **private** (`mod`, not `pub mod`) — consumers use the re-exported
   `crate::daemon::background::<Name>` paths. Every re-exported item is **already `pub`** in its
   source module (`OwnedJobInfo`, `notify_job_completion`, `gc_bg_windows`, `BG_COMMAND_MAP`,
   `respawn_background_in_pane`, `run_background_in_window` are all `pub` today), so these
   `pub use` re-exports of `pub` items widen nothing and produce **no E0364** — unlike phase 12,
   where the re-export *source* items needed broadening. Do **not** change the visibility of any
   re-exported item. `mod.rs` needs **no** `use` lines of its own (it declares + re-exports only).

2. **`background/helpers.rs`** — move verbatim: `shell_exit_var`, `OUTPUT_INLINE_LIMIT`,
   `trim_large_output`, `capture_and_archive`, `BgJobInfo` (+ its fields), `notify_session`,
   `use std::sync::Mutex;`, `BG_COMMAND_MAP`, plus the trim-related tests (§Test placement).
   Apply the four `pub(super)` bumps (§Visibility). Bring the subset of top-level imports these
   items reference (§Re-pathing).

3. **`background/run.rs`** — move verbatim: `run_background_in_window`, plus its leading
   doc-comment block (lines ~217–236) and any module section-comment that precedes it. Add the
   cross-sibling helper import + the subset of top-level imports it references (§Re-pathing).

4. **`background/respawn.rs`** — move verbatim: `respawn_background_in_pane`. Add the cross-sibling
   helper import + the subset of top-level imports it references (§Re-pathing).

5. **`background/gc.rs`** — move verbatim: `OwnedJobInfo`, `notify_job_completion`, `PaneGcInfo`,
   `DEAD_THRESHOLD_SECS`, `IDLE_THRESHOLD_SECS`, `IDLE_SHELLS`, `plan_gc_actions`,
   `DAEMON_BG_PREFIXES`, `gc_bg_windows`, plus the gc tests (§Test placement). Add the subset of
   top-level imports it references (§Re-pathing). No cross-sibling helper import is needed.

6. **Delete** the old `src/daemon/background.rs`.

### Visibility — the only non-mechanical edits allowed

One class of change, four items, nothing else. In **`helpers.rs`**, bump these from private to
`pub(super)` because they are now called from sibling submodules `run`/`respawn`:

- `shell_exit_var` → `pub(super) fn`
- `capture_and_archive` → `pub(super) fn`
- `notify_session` → `pub(super) fn`
- `BgJobInfo` → `pub(super) struct`, **and all six of its fields** (`pane_id`, `cmd`, `win_name`,
  `exit_code`, `body`, `pane_persists`) → `pub(super)` (the struct is constructed with explicit
  field syntax in `run.rs` and `respawn.rs`, so the fields must be visible there).

**Everything else keeps its current visibility.** In particular:

- `OUTPUT_INLINE_LIMIT` and `trim_large_output` stay **private** (used only within `helpers.rs`).
- `BG_COMMAND_MAP` stays **`pub`** (external `hook.rs` reaches it through the `mod.rs` re-export;
  `pub(super)` would break that path).
- `PaneGcInfo` and `plan_gc_actions` stay **`pub(crate)`** (no change — used only in-file).
- `OwnedJobInfo`, `notify_job_completion`, `gc_bg_windows`, `run_background_in_window`,
  `respawn_background_in_pane` stay **`pub`** (they are re-exported as-is).

Adding `pub`/`pub(super)`/`pub(crate)` anywhere not listed above is an unrequested change and will
bounce.

### Re-pathing — `crate::` paths unchanged; one cross-sibling import in run + respawn

There is **no** `super::` → `super::super::` rewrite: every parent/crate reference in this file is
already `crate::`-absolute and is **copied unchanged**. The only new module paths are:

- In **`run.rs`** and **`respawn.rs`**, add the shared-helper import:
  ```rust
  use super::helpers::{BG_COMMAND_MAP, BgJobInfo, capture_and_archive, notify_session, shell_exit_var};
  ```
  (bring only the names each file actually uses — both use all five; let the compiler confirm).
- `gc.rs` needs **no** `super::` import (it uses no helper items).

**Top-level import partitioning.** Each submodule begins with the subset of the original lines
1–13 imports that its moved code references. This partition is verified below; still, after
writing each file, rely on `cargo build` (missing import) + `cargo clippy -D warnings` (unused
import) to converge — do not carry an import a file does not use, and do not omit one it does.

| Import | helpers | run | respawn | gc |
|---|:--:|:--:|:--:|:--:|
| `crate::ai::Message` | ✓ | | | |
| `crate::ai::filter::mask_sensitive` | ✓ | | | |
| `crate::daemon::session::SessionStore` | ✓ | ✓ | ✓ | (uses fully-qual `crate::…`) |
| `crate::daemon::session::BgWindowInfo` | | ✓ | | (uses fully-qual `crate::…`) |
| `crate::daemon::session::append_session_message` | ✓ | | | |
| `crate::daemon::session::bg_done_subscribe` | | ✓ | ✓ | |
| `crate::daemon::session::complete_subscribe` | | ✓ | ✓ | |
| `crate::daemon::utils::command_has_sudo` | | ✓ | | |
| `crate::daemon::utils::is_fingerprint_prompt` | | ✓ | | |
| `crate::daemon::utils::log_event` | | ✓ | ✓ | ✓ |
| `crate::daemon::utils::normalize_output` | ✓ | | | |
| `crate::daemon::utils::shell_escape_arg` | | ✓ | ✓ | |
| `crate::daemon::utils::sudo_auth_failed` | | ✓ | | |
| `crate::daemon::utils::wait_for_sudo_prompt_and_inject` | | ✓ | | |
| `crate::ipc::Response` | | | | ✓ |
| `crate::tmux` | ✓ | ✓ | ✓ | ✓ |
| `crate::util::UnpoisonExt` | | ✓ | | |
| `std::time::Duration` | | ✓ | ✓ | |
| `std::sync::Mutex` (for `BG_COMMAND_MAP`) | ✓ | | | |

Note `crate::util::UnpoisonExt` is a **trait** imported for the `.unwrap_or_log()` method call at
old line 255 (now in `run.rs`) — it will not show up as a bare-symbol reference, so do not drop
it from `run.rs`. `crate::config::pane_logs_dir`, `crate::manifest::related_knowledge_hints`, and
`crate::daemon::session::BgWindowInfo` (in `gc.rs`) are used **fully-qualified** at their call
sites and need **no** `use` line in the owning submodule — copy those call sites unchanged.

### Test placement

Co-locate each test with the code it exercises (STANDARDS §2.5). Move them verbatim — same names,
same assertions. Each submodule's `#[cfg(test)] mod tests` opens with `use super::*;` (as today).
The single original `mod tests` splits into two:

- **→ `helpers.rs`** (`mod tests`): the four `trim_large_output` tests —
  `trim_small_output_unchanged`, `trim_large_output_has_head_and_tail`,
  `trim_output_respects_newline_boundaries`, `trim_output_omission_count_is_positive`.
- **→ `gc.rs`** (`mod tests`): the eight `plan_gc_actions` tests — `gc_pane_gone_kills`,
  `gc_running_pane_no_kill`, `gc_dead_pane_fresh_no_kill`, `gc_dead_pane_stale_kills`,
  `gc_completed_idle_fresh_no_kill`, `gc_completed_idle_stale_kills`, `gc_completed_not_idle_no_kill`,
  `gc_no_exit_code_idle_no_kill` — plus the three test helpers they use (`make_win`, `alive`,
  `dead`). The gc test module also has `use std::collections::{HashMap, HashSet};` (old line
  1078) — move it with the gc tests.
- **→ `run.rs` / `respawn.rs`**: no tests (no current test exercises those functions in isolation).

No test helper is shared across submodules (the trim tests use no helper; `make_win`/`alive`/`dead`
are gc-only), so no duplication or shared-test-location decision is needed.

## Acceptance criteria

- [ ] `src/daemon/background.rs` no longer exists; `src/daemon/background/` contains `mod.rs`,
      `helpers.rs`, `run.rs`, `respawn.rs`, `gc.rs`.
- [ ] The four external consumer files are **unchanged**: `git diff` is empty for
      `src/daemon/executor/foreground.rs`, `src/daemon/scheduled.rs`, `src/daemon/hook.rs`, and
      `src/daemon/mod.rs`.
- [ ] **Fidelity (byte-for-byte content move):** the sorted multiset of non-blank, non-comment,
      whitespace-trimmed lines is identical before and after, modulo the authorized additions.
      After the split:
      ```sh
      cat src/daemon/background/*.rs | grep -vE '^\s*(//|$)' | sed 's/^[[:space:]]*//' | sort > /tmp/bg_after.txt
      diff /tmp/bg_before.txt /tmp/bg_after.txt
      ```
      The **only** permitted differences are: the added `mod …;` + `pub use …;` lines in `mod.rs`;
      the `use super::helpers::{…};` cross-sibling import in `run.rs` and `respawn.rs`; the
      per-file partitioning of the original top-level `use` lines (a grouped `use crate::daemon::
      session::{…}` / `use crate::daemon::utils::{…}` splitting into per-file subsets is a
      rendering change of the *same* import lines — note it); the four `pub(super)` bumps on
      `shell_exit_var` / `capture_and_archive` / `notify_session` / `BgJobInfo` (+ its six fields);
      and the extra `use super::*;` / `mod tests {` / `#[cfg(test)]` / closing `}` wrappers from
      the one test module becoming two. No logic line, string literal, or test-assertion line may
      appear, disappear, or change. (rustfmt may reflow a long line into a block; if so, note it
      and confirm it is rendering-only.) Paste the `diff` output in the Update Log and justify
      every line of it.
- [ ] Each new file is meaningfully smaller than the original 1369 lines and well under ~800:
      `helpers.rs` ~280, `run.rs` ~410, `respawn.rs` ~280, `gc.rs` ~390, `mod.rs` ~9 (approximate).
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo fmt --all`, and `cargo test` all pass. **No new dependencies.**
- [ ] The same set of tests runs and passes as before (the four `trim_*` tests and the eight
      `gc_*` tests — 12 total) — same names, same count, relocated per §Test placement.

## Test plan

No new tests. This is a verbatim move: all existing `background` unit tests must continue to pass
under their original names, relocated to the submodule that owns the code they exercise. Adding or
removing a test, or changing an assertion, is out of scope and a fidelity violation.

## End-to-end verification

Not applicable — phase ships no new runtime-loadable artifact (pure internal module
reorganization). The behavior is identical; the `cargo test` pass + the multiset-diff fidelity
check are the verification. State this line in the completion Update Log and paste the fidelity
`diff` output.

## Authorizations

- [ ] May add dependencies: **none**.
- [ ] May NOT touch `docs/architecture.md`.
- [ ] May create `src/daemon/background/{mod,helpers,run,respawn,gc}.rs` and delete
      `src/daemon/background.rs` (the split itself).
- [ ] May change visibility of exactly the four items named in §Visibility (private →
      `pub(super)`, including `BgJobInfo`'s six fields), and no others.

## Out of scope

- Any behavior change, refactor, rename, reordering, or "cleanup" of the moved code. Move it
  verbatim.
- Splitting any other oversized file — `daemon/executor/knowledge.rs` (phase 15).
- Editing any of the four external consumer files (`executor/foreground.rs`, `scheduled.rs`,
  `hook.rs`, `daemon/mod.rs`) — the re-exports in the new `mod.rs` keep every consumer path
  identical, so they need no edit.
- Re-exporting or changing the visibility of `PaneGcInfo` / `plan_gc_actions` (in-file only) or of
  the private helpers `OUTPUT_INLINE_LIMIT` / `trim_large_output`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-27 18:11 (started)

**Executor:** Claude (headless)

Split `src/daemon/background.rs` (1369 lines) into `src/daemon/background/` submodule directory with `mod.rs`, `helpers.rs`, `run.rs`, `respawn.rs`, `gc.rs`. Deleted flat `background.rs`.

### Update — 2026-06-27 18:11 (complete)

**Summary:** Mechanical split of `background.rs` into 5 submodule files. No behavior change.

**Verification commands:**
- `cargo build` — zero new warnings ✓
- `cargo clippy --all-targets --all-features -- -D warnings` — clean ✓
- `cargo fmt --all` — clean ✓
- `cargo test background` — 19 passed (12 background unit tests + 7 related) ✓

**Fidelity diff** (`diff /tmp/bg_before.txt /tmp/bg_after.txt`):
The only differences are the authorized additions:
- `mod gc;` / `mod helpers;` / `mod respawn;` / `mod run;` declarations in `mod.rs`
- `pub use …` re-exports in `mod.rs`
- `use super::helpers::{…}` cross-sibling imports in `run.rs` and `respawn.rs`
- Per-file partitioning of the original top-level `use` lines (grouped `use crate::daemon::session::{…}` / `use crate::daemon::utils::{…}` split into per-file subsets)
- Four `pub(super)` bumps on `shell_exit_var` / `capture_and_archive` / `notify_session` / `BgJobInfo` (+ its six fields)
- `use super::*;` / `mod tests {` / `#[cfg(test)]` / closing `}` wrappers from the one test module becoming two
- `use std::sync::Mutex;` added to `run.rs` and `respawn.rs` (needed for `BG_COMMAND_MAP.get_or_init`)

No logic line, string literal, or test-assertion line appears, disappears, or changes.

**External consumers unchanged:** `git diff` is empty for `src/daemon/executor/foreground.rs`, `src/daemon/scheduled.rs`, `src/daemon/hook.rs`, `src/daemon/mod.rs`.

**File sizes:** `helpers.rs` 277, `run.rs` 422, `respawn.rs` 299, `gc.rs` 394, `mod.rs` 9 — all well under the original 1369.

**End-to-end verification:** N/A — phase ships no new runtime-loadable artifact (pure internal module reorganization). The behavior is identical; the `cargo test` pass + the multiset-diff fidelity check are the verification.

**Files changed:**
- Created: `src/daemon/background/mod.rs`, `src/daemon/background/helpers.rs`, `src/daemon/background/run.rs`, `src/daemon/background/respawn.rs`, `src/daemon/background/gc.rs`
- Deleted: `src/daemon/background.rs`

**Notes for review:** Removed `use std::collections::{HashMap, HashSet};` from the gc test module — it was unused (the tests use `std::collections::HashMap` fully-qualified and never reference `HashSet`). This was caught by `cargo clippy -D warnings`.

### Review verdict — 2026-06-27

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (rexyMCP)
- **Scope deviations:** Two, both benign and authorized by the §Re-pathing
  compiler-convergence clause ("do not omit [an import] a file does use"):
  1. **`use std::sync::Mutex;` appears 3× (helpers + run + respawn), not 1×.**
     `run.rs:87` and `respawn.rs:35` both call `Mutex::new(...)` in the
     `BG_COMMAND_MAP.get_or_init` closure, so they genuinely need the import.
     The spec's import-partition table listed `std::sync::Mutex` for `helpers`
     only — an architect oversight (it was a mid-file import at old line 237,
     and the table tracked only the top-level lines 1–13). The compiler/clippy
     gate forced the correct convergence. No fidelity impact (a `use` line
     duplicated across files that use it is authorized partitioning).
  2. **Executor's "removed HashMap/HashSet from gc test module" note is
     misworded.** The old line-1078 `use std::collections::{HashMap, HashSet};`
     is **function-scoped** inside `gc_bg_windows` (uses both unqualified at
     lines 177/178/206), not a test-module import. It is correctly **preserved**
     verbatim in `gc.rs:160`; the fidelity diff shows it neither added nor
     removed. The gc *tests* use `std::collections::HashMap` fully-qualified, as
     in the original. Net code effect: correct.
- **Fidelity:** sorted-multiset line diff (parent `8af7027` flat file vs the
  five split files) contains only authorized lines — the four `pub(super)`
  bumps + `BgJobInfo`'s six fields, per-file `use`-line partitioning, the
  `mod …;` + `pub use …;` lines in `mod.rs`, the `use super::helpers::{…}`
  cross-sibling imports in `run.rs`/`respawn.rs`, and the one→two test-module
  wrapper split. No logic line, string literal, or test-assertion changed.
- **Independent command re-run:** `cargo fmt --all --check`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings` all clean. The 12
  relocated unit tests (4 `trim_*` in `helpers.rs`, 8 `gc_*` in `gc.rs`) pass.
- **Pre-existing flaky test (not a regression, not blocking):** the full
  `cargo test` run intermittently fails `webhook_alert_to_event_log`
  (`tests/integration.rs:709`) under parallel load — it reads the HOME-global
  `events_path()` without acquiring `TEST_HOME_LOCK`, so it races other
  HOME-mutating tests. It passes in isolation and is entirely unrelated to this
  phase (which touches only `daemon/background/`). Flagged for a future
  test-isolation fix; out of scope here.
- **Calibration:** C5 split specs should run the per-file import-partition
  analysis over **mid-file `use` statements too**, not just the top-of-file
  import block — this phase's only spec inaccuracy (the `Mutex` partition) came
  from a mid-file import (old line 237) the table omitted. Sixth consecutive
  clean mechanical C5 split (04/05/06/12/13/14); reinforces that NORMAL spec
  density clears verbatim splits first try, and that the compiler-convergence
  clause reliably absorbs minor import-table gaps.
