# Bug 1 on phase-07b: the named-host delete / copy / edit tests never run the op, so a lying remote write ships green

**Severity:** major
**Status:** closed 2026-09-27 — verified fixed at review. The four per-operation tests now drive `run_{create,delete,copy,edit}_with_spawn` on `HostId::Named` and assert the filesystem effect; mutants C, D and E were re-applied independently to production code only and each turned exactly its named test red (2023 passed / 1 failed), tree restored byte-identical. Retyped-command grep 0; lib 2024/0/8 on three runs, not 2025.
**Filed:** 2026-09-27

## What's wrong

Spec Task 5 items 2–4 (`delete_…`, `copy_…`, `edit_on_a_named_host_runs_through_the_shell`)
exist and pass, but none of them calls the operation whose named-host branch
this phase wrote. They feed a command string straight to the transport
through the test helper `run_through_shell` (`src/daemon/executor/file_ops/ops.rs:1215`):

- `delete_on_a_named_host_runs_through_the_shell` (`ops.rs:1256`) **retypes**
  the delete command inline (`ops.rs:1262-1265`) instead of reaching the
  production one in `run_delete_with_spawn` (`ops.rs:540-541`).
- `copy_on_a_named_host_runs_through_the_shell` (`ops.rs:1272`) **retypes**
  the copy command inline (`ops.rs:1280-1285`) instead of reaching
  `run_copy_with_spawn` (`ops.rs:710`).
- `edit_on_a_named_host_runs_through_the_shell` (`ops.rs:1295`) calls the real
  builder `build_remote_edit_cmd`, but not `run_edit_with_spawn`.

`run_delete_with_spawn`, `run_copy_with_spawn` and `run_edit_with_spawn` have
**zero** test callers (`grep -n 'run_\(delete\|copy\|edit\)_with_spawn(' ops.rs`
shows only their definitions and the production wrappers). The tests therefore
exercise 07a's transport with a test-authored string — not 07b's branches.

**Observed at review, by mutation, each applied then restored:**

| Mutant (production code only) | `cargo test --lib` |
|---|---|
| C — delete branch: `then rm -- '{safe_path}' && echo 'DE_OK: Deleted …'` → `then echo 'DE_OK: Deleted …'` (reports success, deletes nothing) | **2024 passed; 0 failed** — survives |
| D — copy branch (the occurrence before `#[cfg(test)]`): `cp -n -- '{safe_src}' '{safe_dst}' && ` → `true && ` (reports success, copies nothing) | **2024 passed; 0 failed** — survives |
| E — edit branch (`ops.rs:119`): `build_remote_edit_cmd(path, old_string, new_string)` → `build_remote_edit_cmd(path, old_string, old_string)` (reports `DE_OK: Edited`, changes nothing) | **2024 passed; 0 failed** — survives |

A remote delete, copy or edit that **reports success without doing the
operation** passes the whole suite.

A second, smaller instance of the same class: `the_write_sentinels_are_unchanged`
asserts `src.contains("cp -n --")` against the file's own source (`ops.rs:1016`),
and that string is also present in the retyped copy at `ops.rs:1283` inside
`mod tests` — so the check is satisfied by test code, which is why mutant D
does not trip it either.

## What should happen

Spec Task 5 item 1 states the property for the per-operation tests: *"the
command reaches `run_in_host_shell`, not a pane"* — i.e. the **operation**,
invoked with a named host, drives its own command through the shell and the
effect lands on the host filesystem. That must hold for all four operations,
so that a broken named-host branch in any of them turns a named test red.

## Root cause

The tests were written against the transport seam (`run_through_shell`) rather
than the operation seam. The operation seam already exists and works in this
file: `a_remote_write_still_prompts_for_approval` (`ops.rs:1123`) drives
`run_create_with_spawn` end to end with `auto_approve_channel()` and an
in-process host spawn (`Box::new(|id| Box::pin(spawn_in_process_host(id)))`).
The delete / copy / edit tests never used it.

## Definition of done

- [x] Mutant C (above), applied with the `patch` tool to the production delete
      branch only, turns **`delete_on_a_named_host_runs_through_the_shell`**
      red; restored afterwards. *(Against the current tree: survives — 2024
      passed.)*
- [x] Mutant D (above), applied to the production copy branch only, turns
      **`copy_on_a_named_host_runs_through_the_shell`** red; restored
      afterwards. *(Against the current tree: survives — 2024 passed.)*
- [x] Mutant E (above), applied to `run_edit_with_spawn`'s named-host branch
      only, turns **`edit_on_a_named_host_runs_through_the_shell`** red;
      restored afterwards. *(Against the current tree: survives — 2024
      passed.)*
- [x] No test in `mod tests` retypes a production write command:
      `awk '/^#\[cfg\(test\)\]/{t=1} t' src/daemon/executor/file_ops/ops.rs | grep -c "rm -- '\|cp -n -- '"`
      → **0** (current: 2).
- [x] `cargo test --lib` → **2024 passed, 0 failed, 8 ignored — not 2025.**
      This fix rewrites four existing tests in place; a rising count means a
      test was added instead of fixed.
- [x] fmt / build / clippy `-D warnings` / full `cargo test` green; the other
      targets unmoved (bug_tracker 6, doc_truth 14, integration 32, isolation 9).
