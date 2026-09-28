# Bug 1 on phase-08f: the live pinky test never runs the never-prompting command it reports, and the Update Log claims it did

**Severity:** major
**Status:** closed 2026-09-28 — verified fixed at review. Test 11's first probe is the privileged clear command (`"su[d]o -k",` → 1) and runs live on pinky in 491 ms, exit 0; `_sentinel_value` → 0; lib 2052/0/10 on three runs, unchanged as pinned. Mutation D re-applied by the reviewer turned test 11 red live (give-up 25 ms, next command failed).
**Filed:** 2026-09-28

## What's wrong

Spec Task 6, test 11 pins the first live probe as the **real** privileged
clear command on `pinky` — "In one shell: `su[d]o -k` → `Ok`, exit 0" — the
live witness that Defect A (a never-prompting command given up on the echo)
is fixed against a real sudo-rs, not only the local fake.

The committed test runs `"true"` instead
(`src/daemon/executor/shell_exec.rs:2497-2509`):

```rust
        let (id, first) = run_in_shell_with_prompts(
            &reg,
            owner.clone(),
            "true",
            5,
            None,
            Some(20),
            ...
        .context("the never-prompting clear on pinky")
```

while its print still names the privileged command
(`shell_exec.rs:2513-2516`):

```rust
        println!(
            "pinky `sudo -k`: {first_elapsed:?} (exit {})",
            first.exit_code
        );
```

So the Update Log's Task 8 entry — `pinky \`sudo -k\`: 286.580077ms (exit 0)`
followed by "The never-prompting clear succeeds in 287 ms (Defect A fixed: no
false give-up on the echo)" — reports a command that never ran. `true` has no
privilege tool in it, so `command_has_…` is false and it takes the **plain**
path: it never touches the sentinel, the classifier or the give-up.

Mechanism of the slip: the helper this test first used,
`run_with_transport(reg, owner, seed_cmd, cmd, …)`, took a **seed**
(`"true"`) and the **command under test**; converting to a direct
`run_in_shell_with_prompts` call kept the seed and dropped the command.

Minor, same file: `run_in_shell_with_prompts` still increments
`PTY_SENTINEL_COUNTER` and binds an unused `_sentinel_value`
(`shell_exec.rs:106-107`) — dead after Task 1 moved the only wrap into
`run_sudo_command`. The leading underscore is what keeps the unused binding
quiet.

## What should happen

Test 11's first probe runs the real privileged clear command on `pinky`
through the prompt path, and its print and the Update Log report what ran.
`run_in_shell_with_prompts` derives no sentinel of its own.

## Root cause

A transcription slip when replacing `run_with_transport` with a direct call
(the resumed run's own Notes asked for exactly that conversion on test 7 and
the executor extended it to test 11), plus the Task 1 refactor leaving its
old counter bump behind. The evidence entry was written from the test's
print, which was never updated.

**Reviewer-verified:** with the first command changed to the privileged clear
command, test 11 passes live on `pinky` — `495.758429ms (exit 0)`, give-up
`337.894443ms`, next command `26.355073ms`. The tree was restored afterwards.
Mutations A and B were also re-applied by the reviewer and turned exactly
their witnesses red (A: tests 5 and 6; B: tests 3 and 6).

## Definition of done

- [x] ``grep -c '"su[d]o -k",' src/daemon/executor/shell_exec.rs`` → **1**
      (current: 0) — test 11's first probe is the privileged clear command.
- [x] ``grep -c '_sentinel_value' src/daemon/executor/shell_exec.rs`` → **0**
      (current: 1).
- [x] Test 11 run live, its output pasted into a new Update Log entry, and
      its first line is the privileged clear command with exit 0 — or
      reported unmet if `pinky` is unreachable.
- [x] `cargo test --lib` → **2052 passed / 0 failed / 10 ignored** — the
      same as now; this fix adds and removes no test.
