# Bug 1 on phase-08a: the primary test is flaky — an unpolled assertion on an asynchronous condition

**Severity:** blocker
**Status:** open
**Filed:** 2026-09-08

## What's wrong

`pty_pause_then_resume_actually_restarts_the_job` — this phase's primary-use
test and its F2 regression guard — fails intermittently. Measured at review
across **11 independent `cargo test --lib` runs: 3 failed, 8 passed** (~27%).
The phase's own acceptance criterion *"`cargo test --lib` reports `0 failed` on
three consecutive runs"* is therefore **not met**; my first three-run sweep went
`FAILED / ok / ok`.

The failing assertion, `src/shell/pty.rs:594`:

```rust
assert!(
    !shell.has_foreground_job(),
    "after STOP the stopped job no longer holds the terminal — the case F2 measured"
);
```

```
thread 'shell::pty::pause_resume_tests::pty_pause_then_resume_actually_restarts_the_job'
panicked at src/shell/pty.rs:594:9:
after STOP the stopped job no longer holds the terminal — the case F2 measured
```

Failing runs also take markedly longer than green ones (13.9 s and 16.9 s
versus a steady 6.9 s), i.e. it fails when the machine is loaded — the signature
of a race, not of a wrong expectation.

## What should happen

The assertion is **correct about the end state** and wrong about *when* it is
reached. Two separate things happen after `SIGSTOP`, in this order and not
atomically:

1. the job enters `/proc` state `T`, and
2. bash reaps the stop notification and takes the terminal back, at which point
   `process_group_leader()` starts returning the shell's own pgid.

The test polls for (1) — `poll_until(… job_state_char(paused) == "T" …)`,
`pty.rs:582-586` — and then asserts (2) **instantly**. Under load, step 2 has
not happened yet and the assertion fires against a state the shell is still
seconds away from.

Every other timing-sensitive wait in this test module already polls:
`wait_for_foreground` (`pty.rs:~540`) and the two `poll_until` calls around
STOP and CONT. This one assertion is the exception.

The property to preserve is the one that makes this test the F2 guard — that
the stopped job is **not** the foreground group, which is why a re-deriving
`CONT` would signal the wrong target. Waiting for that condition asserts it
just as strongly as demanding it instantly, and is what the rest of the module
already does.

## Root cause

`assert!` where the module's own `poll_until` helper belongs. The condition is
produced asynchronously by bash, not by the `signal_foreground("STOP")` call
that returns just before it (`pty.rs:577-594`).

## Definition of done

Each was run against the current tree and returns the "now" value shown.

- [ ] `cargo test --lib` reports `0 failed` on **ten consecutive runs** at the
      default thread count — no `RUST_TEST_THREADS` cap, no skip filter. Ten,
      not three: at the measured ~27% rate a three-run sweep passes cleanly
      about 39% of the time, so three runs cannot distinguish a fix from luck.
      Paste all ten `test result:` lines into the Update Log. (Now: fails
      within the first four runs, reproduced twice at review.)
- [ ] The foreground-handoff condition is **polled, not asserted instantly**.
      The bare assertion line at `pty.rs:594` is gone:
      `grep -cE '^\s+!shell\.has_foreground_job\(\),' src/shell/pty.rs` → **0**
      (now `1`), while the check itself survives inside a `poll_until` —
      `grep -c 'has_foreground_job' src/shell/pty.rs` stays **5 or more** (now
      `5`). *(Do not delete the check; it is what makes this test the F2
      guard.)*
- [ ] **The guard still fails against the pre-08a `CONT`.** Substitute the
      `"CONT"` arm (`pty.rs:364-372`) with the re-deriving form —
      `self.paused_pgid.take(); if let Some(pgid) = current { self.signal_group(pgid, sig)?; }`
      — and confirm `pty_pause_then_resume_actually_restarts_the_job` is the
      only red test in `shell::pty::pause_resume_tests::`; restore and confirm
      green. Paste both `test result:` lines. *(Verified at review against the
      current tree: red 2 passed / 1 failed, green 3 passed — the fix must not
      cost this.)*
- [ ] `cargo test --lib shell::pty::` reports **16 or more** passing,
      `0 failed`, and the other module counts are unchanged: `shell::backend::`
      **9**, `shell::client::` **9**, `shell::proto::` **5**, `shell::host::`
      **8**, `shell::registry::` **17**, `shell::session::` **6**,
      `shell::launch::` **8**, `shell::log::` **13**, `shell::screen::` **11**.
- [ ] No stray jobs left on the host after the ten runs:
      `ps -eo args | awk '/^sleep 30$/ {n++} END{print n+0}'` → **0**.
      **Do not** clean up with `pkill`, `killall`, or any signal to a process
      group you did not create.
- [ ] All four gates pass.
