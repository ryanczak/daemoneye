# Bug 2 on phase-07c: a test that passes with the whole phase deleted

**Status:** fixed
**Severity:** minor
**Filed:** 2026-09-08

## What's wrong

`shell_exec_registry_records_are_leased_and_released_like_session_tests`
(`src/daemon/executor/shell_exec.rs:295-315`) inserts one record into a fresh
`ShellRegistry` and asserts the map then holds one entry:

```rust
with_shells(&reg, |store| { store.insert("s4".to_string(), ShellRecord { … }); });
let records = with_shells(&reg, |store| store.len());
assert_eq!(records, 1, "the seeded record is visible through the API used");
```

It exercises `HashMap::insert` and `with_shells` — both shipped in phase-06a —
and touches no code this phase wrote. Delete `shell_exec.rs`'s entire
production body and this test still passes. Its name also describes behaviour
that does not occur anywhere in it: nothing is leased and nothing is released.

It is one of the seven that `daemon::executor::shell_exec::` reports, so it
inflates the count the acceptance criterion reads.

## What should happen

Either the test asserts something about this phase's code, or it goes. If a
registry-interaction test is wanted here, the untested behaviour worth pinning
is the **cap path**: `run_in_shell_with` with `max_per_owner = 1` against an
owner that already holds a live shell must return `Err`, not panic and not
silently exceed the cap — the executor branch surfaces that error to the model
and nothing currently covers it.

STANDARDS §3.2 lists what does *not* need a test; the converse holds too — a
test that cannot fail when the code under test is wrong is not coverage.

## Root cause

Not a mechanism failure — a test written to reach a count rather than to pin a
behaviour. The phase's test plan named six tests; this is a seventh, and it is
the only one of the three extras that asserts nothing about the phase.

## Definition of done

- [ ] `grep -c 'shell_exec_registry_records_are_leased_and_released_like_session_tests'
      src/daemon/executor/shell_exec.rs` → **0** (now: `1`).
- [ ] Either `daemon::executor::shell_exec::` reports **6 passed; 0 failed**
      (test removed), or it reports **7 passed; 0 failed** with a replacement
      test that fails when the behaviour it names is broken — state which you
      chose and why in the Update Log.
- [ ] If a replacement is written: it fails against a deliberately broken
      version of the code it covers, and the Update Log quotes that red run.
