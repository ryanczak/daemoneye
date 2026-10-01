# Bug 1 on phase-03a: `ShellList` drops every shell's `interactive`, `dedicated` and `exited_unix`

**Severity:** major
**Status:** closed 2026-10-01 — fixed in `f308a8bc`; the new test passes and Mutation C reproduced red at review
**Filed:** 2026-10-01

## What's wrong

`handle_shell_list` (`src/daemon/server/shells.rs:23-47`) copies only `id`,
`host`, `owner` and `started_unix` out of each registry record. It then
builds every `ShellInfo` with three hardcoded values:

```rust
            interactive: false,
            dedicated: false,
            exited_unix: None,
```

**Observed at review.** In a fresh `ShellRegistry`, `s0` was marked interactive
(`mark_interactive`), and `s1` was spawned `dedicated` with `exited_unix` set
to `Some(42)`. `handle_shell_list` reported:

```
PROBE s0 interactive=false dedicated=false exited=None
PROBE s1 interactive=false dedicated=false exited=None
```

A client listing shells would show a dead shell as live, and an
ssh-captured or background-dedicated shell as an ordinary one. Row 04's
`/shells` state column is built on exactly these fields.

## What should happen

Task 1 / Task 3: each `ShellInfo` is mapped **from its `ShellRecord`**. All
eight fields carry the record's values; only `last_command` comes from the
meta index.

## Root cause

The collector closure's tuple (`src/daemon/server/shells.rs:23-35`) omits the
three fields, so nothing outside the lock can fill them in. Test 2
(`shell_list_reports_every_shell_with_its_last_command`) cannot see it. Its
shells are all non-interactive, non-dedicated and live, which are exactly the
hardcoded values.

## Definition of done

- [x] A new test `shell_list_carries_each_shells_flags` passes. It sets up
      `s0` interactive (via `crate::shell::mark_interactive`), and `s1`
      spawned with `dedicated = true` and `exited_unix` set to `Some(42)`
      through `with_shells(&reg, |store| …get_mut…)`. It asserts all six
      values on the wire. Against the current tree that test does not exist
      (`0 passed`).
- [x] Mutation C (put the three hardcoded values back) turns that test red.
- [x] `cargo test --lib` → 2105 passed / 0 failed / 14 ignored on three runs.
