# Bug 1 on phase-21e: the exited filter leaks into the prompt's shells block and `find_in_shells`, and `top`'s test re-implements the filter

**Severity:** major
**Status:** open
**Filed:** 2026-11-12

## What's wrong

1. **The filter is in the shared helper.** `resolve_scope`
   (`src/daemon/executor/knowledge/shell.rs`) now drops records with
   `exited_unix` set, in both scope arms. `resolve_scope` has three callers:
   `list_shells`, `shell_context_block_in` (the prompt's ambient
   `[SHELLS]` block) and `find_in_shells` (`shell_watch.rs`). So the
   phase changed two surfaces the spec kept unchanged:
   - Behaviour 6 says "The prompt's shells context block is not touched".
   - `find_in_shells` is neither in Behaviour 4 nor authorized. An exited
     shell's recording can no longer be searched by it. Asking "which shell
     printed the error?" right after the detach grace closed the shell that
     printed it now misses that shell.
   The Update Log names `find_in_shells` "inheriting the same hiding" as
   intended. It wasn't in the spec.
2. **`top`'s test doesn't run `top`'s code.** `shells_panel_omits_exited_rows`
   (`src/cli/commands/top_load.rs`) filters through `panel_rows`, a
   test-module copy of the predicate in `shells_panel`'s match arm. Deleting
   the production `.filter` leaves the test green. This is the same defect
   as bug-21d-1: a test that drives a copy of the code it means to pin.

## What should happen

- `list_shells` omits exited rows in both scopes, as today. The filter
  belongs to `list_shells` alone, not to `resolve_scope`.
  `shell_context_block_in` and `find_in_shells` see exactly what they saw
  before this phase.
- The rows `top`'s Shells panel draws come from one production function
  (for example a `pub(crate)` helper that both `shells_panel` and the test
  call). `shells_panel_omits_exited_rows` calls that function. No
  `#[cfg(test)]` copy of the predicate remains.

## Root cause

The filter was placed at the shared collection point instead of at
`list_shells`' own rows. The test needed a seam that `shells_panel` (an
IPC call) did not offer, and a copy was written instead of a seam.

## Definition of done

- [ ] `awk '/^fn resolve_scope/,/^}/' src/daemon/executor/knowledge/shell.rs | grep -c exited_unix`
      prints 0. Today 2.
- [ ] `shell_context_still_lists_exited_shells` passes. A registry with one
      live and one exited record gives a `shell_context_block_in` block
      that names both, the exited one carrying `[exited]`. Against today's
      tree the exited one is missing.
- [ ] `list_shells_omits_exited_shells` and
      `list_shells_of_only_exited_shells_says_so` still pass.
- [ ] `grep -c 'fn panel_rows' src/cli/commands/top_load.rs` prints 1, and
      that one definition is outside `mod tests`. `shells_panel` calls it,
      and `shells_panel_omits_exited_rows` calls it. Today the only
      definition is inside `mod tests`.
- [ ] `cargo test` reports `passed=2894` (one new test).
