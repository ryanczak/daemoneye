# Bug 1 on phase-06a: a `host` override is refused for the runbook's `ssh_target`

**Severity:** major
**Status:** open
**Filed:** 2026-11-26

## What's wrong

`narrow_to_caller` (`src/daemon/executor/knowledge/ghost.rs`) checks the
child runbook's `ssh_target` against the narrowed `hosts:` list **whatever
the `host` argument is**. `spawn_ghost` calls it before the `host` check,
and that later check replaces `ssh_target` with the named host.

So a legitimate delegation is refused. The reviewer reproduced it with a
scratch test, since reverted:
- **Setup:** `[hosts.db01]` and `[hosts.db02]` configured; a caller with
  `hosts: [db01]`; a runbook with `ssh_target: db02`.
- **Call:** `spawn_ghost(…, host: Some("db01"), Some(&caller), …)`.
- **Result:** `Error: cannot spawn this specialist. Its ssh_target 'db02'
  is outside your host fence (db01).` The child would never have run on
  `db02`.

## What should happen

The phase spec, item 2, says: "**`ssh_target`, when `host` is omitted**".
With `host` given, the existing `spawn_host_target` check governs. It
already tests the named host against the narrowed child's `hosts:` list
and the caller's. The runbook's own `ssh_target` is replaced, so it is not
checked.

The narrowing of hosts, scripts, flags and tool policy still runs for
every ghost caller, `host` given or not.

## Root cause

`narrow_to_caller` does not know whether `host` was passed, so it cannot
skip the `ssh_target` check.

## Definition of done

- [ ] `spawn_ghost_lets_a_host_override_replace_the_runbook_ssh_target`
      passes. Its setup and call are the reproduction above, and it
      asserts that the outcome does **not** contain
      `cannot spawn this specialist`. It is red today.
- [ ] `narrow_to_caller_refuses_an_ssh_target_outside_the_caller_fence`
      and `spawn_ghost_refuses_a_child_outside_the_caller_fence` still
      pass.
