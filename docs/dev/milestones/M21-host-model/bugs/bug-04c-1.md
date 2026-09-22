# Bug 1 on phase-04c: the new seam test is itself order-dependent

**Severity:** minor
**Status:** closed 2026-09-22 — both findings verified fixed at review. The guard audit now reads **GUARDED on all six** seam-touching tests: `swap_in_restores_the_previous_registry_on_drop` takes `crate::test_home_guard()` as its first line, with a doc comment naming the lock (not the `$HOME` swap) as the reason. `spawn_dedicated_for` is **byte-identical to its pre-phase form** at `5afbf15e` — the stronger of the two allowed branches — so no declaration is needed. Preservation held: lib 1943/0/5 on three runs, all four gates green, the eight named tests pass, the phase's own E2E capture reproduces byte for byte and `PASTE MATCH` still prints.
**Filed:** 2026-09-22

## What's wrong

The phase's work is correct and every acceptance criterion is met — this
review reproduced the whole § End-to-end verification capture byte for byte
and re-ran all four gates clean. Two defects remain, both small.

**1. `swap_in_restores_the_previous_registry_on_drop` takes no home guard.**

Six tests touch `open_shell_test_seam`'s `REPLACE` global. Five take
`lifecycle_home()` (which takes `crate::test_home_guard()`), which is what
serializes them. The one added by this phase does not:

```
GUARDED    open_shell_on_an_unknown_host_names_the_host_not_the_shell_limit
GUARDED    open_shell_without_a_host_records_the_local_host
GUARDED    open_shell_local_result_is_unchanged
GUARDED    open_shell_rejects_a_malformed_host_name_before_any_spawn
UNGUARDED  swap_in_restores_the_previous_registry_on_drop
GUARDED    open_shell_without_a_swap_uses_the_daemon_registry
```

At `src/daemon/executor/knowledge/shell.rs:3451-3476` it both **reads** the
shared global as a precondition —

```rust
assert!(
    open_shell_test_seam::swapped().is_none(),
    "precondition: no swap is in scope (this test must not depend on another)"
);
```

— and **mutates** it twice. Its own assertion message states the property it
does not actually hold: nothing stops one of the five guarded tests from
holding a swap while this precondition runs, and nothing stops this test's
`swap_in` / `Drop` from landing inside a guarded test's swap window.

**This is the defect the phase existed to remove, reintroduced in the test
written to prove it was removed.** Phase-04b's review deferred the seam fix
here in these words: *"It becomes a real order-dependent flake the moment a
test calls `open_shell` without swapping — which row 04c will do. Fix it
there, as a real guard, before adding `run_terminal_command { host }`
tests."* Task 1's RAII guard is correct and proven; the new test's missing
home guard is the same class one level up.

**Not reproduced, and that is stated plainly.** The executor recorded a
1-in-80 full-suite blip whose failing name was never captured, then ran 82
green. This review ran ~50 targeted runs (including a deliberately widened
400 ms race window across the seam tests at `--test-threads=8`) and 12 full
lib runs — all green. **The structural argument stands on the audit above,
not on a reproduction**, and this bug does not claim the observed blip was
this test.

**2. `spawn_dedicated_for` was edited, though § Out of scope said it was not
to be, and the Update Log says it was not.**

`src/shell/session.rs:222` now calls `spawn_for_on_host(reg, owner,
max_per_owner, HostId::Local, true, spawn)` where it called `spawn_for(reg,
owner, max_per_owner, true, spawn)`. Behaviour is identical — `spawn_for` is
itself that one-line `Local` delegation — so nothing is broken. But:

- § Out of scope named it: *"**`spawn_dedicated_for` and the background
  path's no-reuse rule**. Background commands deliberately never reuse; that
  is unchanged and **untouched**."*
- The completion notes claim: *"`spawn_for` remains a one-line `Local`
  delegation used by `spawn_dedicated_for`, untouched per § Out of scope."*
  Its own diff contradicts this. `spawn_for` survives and has ten callers
  elsewhere, but `spawn_dedicated_for` is no longer one of them.

## What should happen

1. The new seam test serializes against the other five the same way they
   serialize against each other, so its precondition is a fact rather than a
   hope. `STANDARDS.md` §3.3 requires tests to be deterministic.
2. Either `spawn_dedicated_for` goes back to `spawn_for` (it is out of
   scope and the change buys nothing), or the Update Log states the edit and
   why. What must not stand is the Update Log asserting the opposite of the
   diff.

## Root cause

**1.** The five pre-existing tests acquire the guard incidentally — they need
`$HOME` for `Config::ensure_dirs()`, and `lifecycle_home()` bundles the home
setup with `test_home_guard()`. The new test needs no `$HOME`, so there was
no prompt to reach for the helper, and the lock it also provides was lost
with it. The guard's two jobs are bundled in one helper and only one of them
is visible at the call site.

**2.** Task 3 replaced `spawn_for` with `spawn_for_on_host` in
`shell_for_owner`'s create step, which removed `spawn_for` from
`session.rs`'s import list (`session.rs:14`). The other in-file caller,
`spawn_dedicated_for`, then had to change too — or the import had to stay.
Changing it was the path of least resistance and crossed a scope line.

## Definition of done

- [ ] The new test serializes on the same guard as the other five seam tests:

      ```sh
      awk '/fn swap_in_restores_the_previous_registry_on_drop/,/^    }$/' \
        src/daemon/executor/knowledge/shell.rs \
        | grep -c 'test_home_guard()\|lifecycle_home()'
      ```

      → **≥ 1**. Confirmed **FAILING** at filing: `0`. Either helper is fine;
      `test_home_guard()` alone is sufficient since the test needs no `$HOME`.
- [ ] `spawn_dedicated_for` is off the out-of-scope list or declared:

      ```sh
      awk '/pub async fn spawn_dedicated_for/,/^}$/' src/shell/session.rs \
        | grep -c 'spawn_for_on_host'
      ```

      → **0**, **or** it stays `1` and an Update Log entry names the edit and
      its reason. Confirmed **FAILING** at filing: `1`, undeclared.
- [ ] No claim in the Update Log contradicts the commit's diff.
- [ ] Preservation — the phase's own results are undisturbed:
      `cargo test --lib` → **1943 passed / 0 failed / 5 ignored**; all four
      gates green; the eight named tests still pass.
