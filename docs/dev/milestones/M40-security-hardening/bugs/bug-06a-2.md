# Bug 2 on phase-06a: two doc regressions

**Severity:** minor
**Status:** open
**Filed:** 2026-11-26

## What's wrong

1. `narrow_to_caller` was inserted between `spawn_ghost`'s `///` doc
   comment and `spawn_ghost` itself, in
   `src/daemon/executor/knowledge/ghost.rs`. Rustdoc joins consecutive
   `///` lines. So `narrow_to_caller`'s doc now begins "Spawn a ghost
   session from a runbook…", and `spawn_ghost` has no doc at all.
2. The `spawn_ghost_shell` row of CLAUDE.md's Current AI tools table lost
   its `host?` sentence, which says the specialist runs on a configured
   host or `"local"`. The tool still takes `host`.

## What should happen

1. `spawn_ghost`'s doc comment sits directly above `spawn_ghost` (and its
   `#[allow]`). `narrow_to_caller` has only its own doc.
2. The row keeps the `host?` sentence, and adds the narrowing sentence.

## Definition of done

- [ ] This prints **0** (it prints 1 today):
      `grep -B12 "pub(super) fn narrow_to_caller" src/daemon/executor/knowledge/ghost.rs | grep -c "Spawn a ghost session from a runbook"`
- [ ] This prints **1** (it prints 0 today):
      ``grep -c '^| `spawn_ghost_shell` .*host?' CLAUDE.md``
