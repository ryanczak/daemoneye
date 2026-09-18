# Bug 1 on phase-09d: ghost background script-path resolution was deleted with the retry block

**Severity:** major
**Status:** open
**Filed:** 2026-09-17

Every acceptance criterion of phase-09d is met, all four gates are green, and
the milestone's `%N = 0` exit criterion is achieved. This bug is a **behaviour
regression in a path the phase does not own**, invisible to every gate.

## What's wrong

`GhostPolicy::resolve_command` (`src/daemon/policy.rs:86`) now has **zero
production callers.**

```sh
# before the phase (eb37607e)
$ git grep -n 'resolve_command' eb37607e -- src/ | grep -v 'policy.rs\|policy_tests.rs'
eb37607e:src/daemon/executor/foreground.rs:1071:            resolved_retry_cmd = policy.resolve_command(cmd);
eb37607e:src/daemon/executor/foreground.rs:1134:        resolved_cmd = policy.resolve_command(cmd);

# now
$ grep -rn 'resolve_command' src/ --include=*.rs | grep -v 'policy.rs\|policy_tests.rs'
(nothing)
```

**`:1071` was inside the `retry_pane` block and was correctly removed with it.**
**`:1134` was in `run_background`'s main path and should have stayed.** The diff
removed this block, which has nothing to do with `target_pane` or
`retry_in_pane`:

```rust
-    // Ghost shells: resolve bare/relative script names to absolute path.
-    let resolved_cmd;
-    let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
-        resolved_cmd = policy.resolve_command(cmd);
-        resolved_cmd.as_str()
-    } else {
-        cmd
-    };
```

**Observed behaviour:** a ghost shell or scheduled job that runs a background
command naming a whitelisted script by bare or relative name (`fix.sh`,
`./fix.sh`) no longer has it rewritten to the absolute
`~/.daemoneye/scripts/fix.sh`, and a policy with `run_with_sudo` no longer gets
its `sudo` prefix applied. The command is passed through verbatim and will fail
to find the script, or run without the sudo the policy requires.

## What should happen

`run_background` must resolve ghost script paths exactly as it did before, on
the non-retry path. The behaviour `resolve_command` provides — strip a leading
`sudo`, rewrite a whitelisted bare/relative script name to its absolute (or
`~`-prefixed, for `ssh_target`) path, re-apply `sudo` when `run_with_sudo` is
set — is specified and unit-tested in `src/daemon/policy_tests.rs` (25 tests
covering bare name, relative path, args, absolute passthrough, sudo prefix and
remote tilde paths). None of them exercise the **call site**, which is why its
deletion was invisible.

Phase-09d § Out of scope is explicit that this phase touches the AI's *choice*
of pane, not execution behaviour: "Ghost sessions and the opt-out still execute
in panes; this phase stops the **AI** from choosing which."

## Root cause

`run_background` contained **two** `resolve_command` blocks — one inside the
`retry_pane` early-return path (`:1071`) and one in the main path (`:1134`).
They are near-identical, five lines apart in shape, and differ only by their
binding name (`resolved_retry_cmd` vs `resolved_cmd`). Removing the retry block
correctly took the first; the second was removed with it.

No gate catches this. `resolve_command` is `pub` on `GhostPolicy`, so
`dead_code` does not fire; its 25 unit tests call it directly and still pass;
and no test covers `run_background`'s call site:

```sh
$ grep -rn 'run_background' src/ --include=*.rs | grep -i 'test'
(nothing)
```

## Definition of done

Each was run against the current tree (HEAD `1ecb2f46`) and **fails** there.

- [ ] `grep -rn 'resolve_command' src/ --include=*.rs | grep -v 'policy.rs\|policy_tests.rs' | wc -l` → **1** (now 0) — the main-path call restored in `run_background`, and **only** that one; the retry-path call stays deleted.
- [ ] A regression test named `run_background_resolves_ghost_script_paths` exists and passes: `grep -c 'fn run_background_resolves_ghost_script_paths' src/` → **1** (now 0). It must pin the **call site**, not `resolve_command` itself — construct a `GhostPolicy` whose whitelist contains a script, pass a bare name through whatever seam `run_background` exposes, and assert the resolved absolute path reaches execution. If `run_background` has no testable seam, say so in the Update Log and pin the behaviour at the nearest seam that exists, naming it.
- [ ] Mutation check: delete the restored `resolve_command` call again and confirm `run_background_resolves_ghost_script_paths` goes **red**; revert and confirm green. Paste both `test result:` lines. This is the whole point — the deletion must stop being invisible.
- [ ] `cargo test --lib` reports **1879 passed** (now 1878) — exactly one new test.
- [ ] All four gates stay green, and every phase-09d acceptance criterion still passes — in particular `grep -o '%[0-9]' src/ai/tools/defs.rs | wc -l` → **0** and the tool counts stay **42 / 28**.

**Update — 2026-09-17 21:20 (fixed by re-dispatch):** the main-path
`resolve_command` block is restored in `run_background`
(`src/daemon/executor/foreground.rs:952`), the retry-path call remains
deleted, and the call site is pinned by
`run_background_resolves_ghost_script_paths` (real private-tmux invocation;
asserts the audit record carries the resolved absolute path and the script's
side effect landed; mutation-checked red→green with both `test result:` lines
in the phase's Update Log). `cargo test --lib` → 1879 passed / 4 ignored.
Commit `51710d99`.
