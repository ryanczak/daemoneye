# Bug 1 on phase-17a: `an_exited_detached_child_is_reaped` passes with the child dropped unwaited, so B12 is untested

**Severity:** major
**Status:** open
**Filed:** 2026-11-07

## What's wrong

`an_exited_detached_child_is_reaped` (`src/shell/launch.rs` tests) passes
against the pre-phase code. The reviewer replaced the reaper thread in
`spawn_detached` with the original `drop(child);` and ran the test:

```
== mutation: child dropped unwaited (pre-phase behaviour)
test shell::launch::tests::an_exited_detached_child_is_reaped ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2695 filtered out; finished in 10.03s
```

The test cannot fail, for two reasons:

1. **It never spawns `sh`.** It calls `spawn_detached("s1", "sh", &["exit 0"], …)`.
   That re-invokes `current_exe()`, which under `cargo test` is the test
   binary itself, with `shell-host --id s1 --shell sh -- exit 0`. So the
   child is the test harness rejecting unknown flags, not `sh -c 'exit 0'`.
2. **Its zombie match never matches.** It scans `ps -o pid=,stat=,args=`
   for a row whose third whitespace token is exactly `exit`. A zombie's
   args column shows as `[name] <defunct>`, so no row ever matches, zombie
   or not.

The test also takes 10 s, because it waits out `await_ready`'s readiness
timeout.

The phase's Spec asked for something else: spawn `sh -c 'exit 0'`
**through the waiting mechanism**, then check `/proc/<pid>/stat` for that
pid. The test does neither.

## What should happen

The behaviour B12 fixes must be pinned by a test that fails on the
pre-phase code. Something waits on each spawned shell-host child, so an
exited child is not left in state `Z`. The test must:

- know the pid of a child that exits at once;
- hand that child to **the same waiting code `spawn_detached` uses**;
- confirm through `/proc/<pid>/stat` that it is not left a zombie.

## Root cause

The waiting code is inlined in `spawn_detached`, after a `spawn` of
`current_exe()`. So no test can give it a child of its own choosing. The
test then tried to go through `spawn_detached` end to end, where both its
child and its zombie detection are wrong.

## Definition of done

- [ ] The test spawns a real `sh -c 'exit 0'` child, keeps its pid, and
      passes the `std::process::Child` to the waiting code `spawn_detached`
      itself calls. That code is no longer inlined and has no test-only
      copy. Within 2 s, `/proc/<pid>/stat` must be absent or its state
      field must not be `Z`.
- [ ] **Mutation check, pasted as real output:** make the waiting code
      drop the child without waiting, and the test fails. Restore it, and
      the test passes.
- [ ] `cargo test --lib an_exited_detached_child_is_reaped` reports
      `finished in` under 3 s. Today it is 10.03 s.
- [ ] `docs/architecture.md`: the phase changed "The two are independent
      lifecycles" to "The tests are independent lifecycles". Restore "The
      two are", or reword so that the sentence names the lifecycles it
      compares.
- [ ] `cargo test` still reports `passed=2775 failed=0`. This is a test
      fix, not a new test, so the count must **not** change.
