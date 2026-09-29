# Bug 1 on phase-08e: the connect timeout never restarts after an answer, so a slow login is killed after the password was typed

**Severity:** major
**Status:** open
**Filed:** 2026-09-28

## What's wrong

Spec Task 2.1: "on `Send(bytes)` → `write_input(&bytes)` and
`answered_at = buf.len()`, **and restart the timeout**". The doc comment on
`CONNECT_TIMEOUT` (`src/daemon/executor/shell_exec.rs:506`) says the same:
"the timeout is restartable so an answered prompt does not eat the budget".

The code does record `last_answer = Instant::now()` (`:612`), but the check at
`:580` ORs it with the **unrestarted** start time:

```rust
        if Instant::now().duration_since(started) > timeout
            || Instant::now().duration_since(last_answer) > timeout
```

Since `last_answer >= started`, the first clause always fires first — the
condition is simply "30 s since spawn". Answering never buys time.

**Reproduced by the reviewer** (throwaway test, reverted): fake ssh
`sleep 0.4; printf "u@h's password: "; read -r pw; sleep 0.5; touch <flag>; exit 0`,
one `CredentialResponse`, `timeout = 700ms`. The answer lands at ~0.42 s; the
fake connects ~0.5 s later — well inside 700 ms of the answer — but:

```
  left: Err("ssh to host pinky did not connect within 700ms")
 right: Ok(())
test result: FAILED. 0 passed; 1 failed; …
```

In production: `credential_round_trip` blocks while the user types, and each
wrong password costs ssh ~3.8 s (measured, § Current state 3). A user who
takes ~25 s over a first prompt and gets it wrong once has ssh killed
**mid-authentication, after the credential was already written**, and is
told the host "did not connect within 30s".

## What should happen

The deadline is measured from the later of spawn and the last answer
(spec Task 2.1, and the code's own doc comment). A login that completes
within `timeout` of its last answer returns `Ok(())` however long the user
took before answering.

## Root cause

`shell_exec.rs:580` — the `started` clause is never reset and dominates the
`last_answer` clause.

## Definition of done

- [ ] A new test in `connect_tests`,
      `an_answered_prompt_restarts_the_connect_timeout`, drives
      `drive_connect` over a fake that answers late relative to spawn but
      connects within `timeout` of the answer (the shape above: prompt after
      ~0.4 s, connect ~0.5 s after the answer, `timeout` 700 ms) and asserts
      `Ok(())`. **Confirmed FAILING against the current tree** (the probe
      above is that test).
- [ ] The existing "never answered" timeout still fires: a fake that prints
      nothing and sleeps past `timeout` returns the `did not connect within`
      error (existing behaviour; pin it in the same test module if no test
      covers it).
- [ ] `cargo test --lib` → 0 failed, ignored count **12** (unchanged).
