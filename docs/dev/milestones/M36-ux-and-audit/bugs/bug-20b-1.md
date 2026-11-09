# Bug 1 on phase-20b: the setup report and the path audit don't know about config-reference

**Severity:** major
**Status:** open
**Filed:** 2026-11-09

## What's wrong

Behaviour 2 says: "Every list of seeded memory names (seeds, the setup
report, the path audit) includes it." Only `src/config/seeds.rs` was
updated. Two lists were not:

- `src/cli/commands/setup.rs`, `seeded_knowledge`. This is the list
  `daemoneye setup` prints under "Seeded memories" and
  `--overwrite-memory` prints under "Overwriting built-in memories". The
  memory *is* written, but the report never names it, so an operator can't
  tell it was seeded or rewritten.
- `src/config/path_audit.rs`, the `assets` table in
  `all_assets_audit_clean_with_empty_quarantine`. Every other seeded
  knowledge memory is audited for stale `~/.daemoneye/` path literals.
  The new memory, the asset with the most paths in it, is not, so a
  stale path in it would pass the gate.

## What should happen

Both lists name `config-reference` beside `agent-runtime-layout`. The path
audit runs over `CONFIG_REFERENCE_MEMORY` with an empty quarantine and is
clean. If it reports findings, fix the memory's wording, not the audit.

## Root cause

The new seed was added to the two lists in `seeds.rs` but not to the two
other places that enumerate the seeded knowledge memories by name.

## Definition of done

- [ ] `grep -c 'config-reference' src/cli/commands/setup.rs` prints 1 or
      more. **It prints 0 today.**
- [ ] `grep -c 'CONFIG_REFERENCE_MEMORY' src/config/path_audit.rs` prints 1
      or more. **It prints 0 today.**
- [ ] `all_assets_audit_clean_with_empty_quarantine` passes with the new
      asset in its table.
