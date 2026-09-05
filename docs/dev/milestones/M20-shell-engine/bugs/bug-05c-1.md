# Bug 1 on phase-05c: the HOME/PATH race is only half-fixed — `shell::backend::` still spawns unguarded

**Severity:** major
**Status:** resolved 2026-09-05 (verified at review: 0 failures in 10 runs, on top of 22 clean at escalation)
**Filed:** 2026-09-05

## What's wrong

Round 2 applied the round-2 guard exactly as prescribed — four
`let _home = crate::test_home_guard();` lines in `src/shell/pty.rs`, verified
present — and `cargo test` (the configured gate) is green **10 runs out of
10**. But the race it was meant to close is still live under a narrower, and
entirely ordinary, filter:

```
cargo test --lib shell::     3 FAILED in 18 runs (~17%)
```

The failing test **moved** rather than disappearing. It is no longer
`shell::pty::`; it is now `shell::backend::`, with the identical signature:

```
thread 'shell::backend::tests::backend_fans_one_chunk_out_to_subscriber_log_and_screen'
  panicked at src/shell/backend.rs:178:68:
bash available: spawning shell
Caused by:
    0: spawn_command failed
```

and, on another run,
`shell::backend::tests::backend_resize_replaces_the_screen_and_does_not_reflow`.

**This is the architect's incompleteness, not an executor error.** The round-2
block named only the four `PtyShell::spawn("bash", (24, 80))` sites in
`pty.rs`. It missed that the `backend` tests reach the same PATH lookup through
a different door — `PtyBackend::spawn` → `PtyShell::spawn` — from a helper the
grep for `PtyShell::spawn` never showed:

```rust
// src/shell/backend.rs:177
fn spawn_backend(dir: &tempfile::TempDir) -> Arc<PtyBackend> {
    PtyBackend::spawn("bash", 24, 80, 100, &cast_path(dir), 0).expect("bash available")
}
```

The architect's own verification was also too weak to catch it: five clean
`shell::` runs at a ~17% per-run failure rate is a ~40% chance of seeing
nothing. It saw nothing.

## What should happen

Every test that spawns a PTY must hold `test_home_guard()` across the spawn, so
no `PtyShell::spawn` overlaps a `std::env::set_var("HOME", …)` from the
`launch_*` tests. `pty.rs`'s four sites are done; `backend.rs`'s helper is the
one remaining door.

## Root cause

Unchanged from the escalation, and now fully enumerated. `std::env::set_var`
mutates the process-global environment; glibc's `setenv` may reallocate
`environ`. A concurrent `getenv("PATH")` — which is what `CommandBuilder`
performs while resolving the bare name `bash` inside `PtyShell::spawn`
(`src/shell/pty.rs:188`) — can then read a torn pointer and find nothing, so
the spawn reports `No such file or directory`. `test_home_guard()` serializes
only the tests that *take* it. Guarding the writers is not enough; every
reader that spawns must take it too.

Confirmed by measurement rather than inference:

| tree | `cargo test --lib shell::` |
|---|---|
| as landed (pty.rs guarded only) | **3 failed / 18 runs** |
| with `spawn_backend` also guarded | **0 failed / 22 runs** |

At a 17% per-run rate, 22 consecutive clean runs by chance is ~1.7%.

## Definition of done

Confirmed FAILING against the current tree at filing time — each value below is
what the tree returns **now**.

- [ ] `grep -c 'let _home = crate::test_home_guard();' src/shell/backend.rs`
      → **1** (now `0`).
- [ ] `grep -c 'let _home = crate::test_home_guard();' src/shell/pty.rs`
      → **4**, unchanged. Do not add more there.
- [ ] `cargo test --lib shell::` run **ten times in a row**, every run
      reporting `0 failed`. Paste all ten `test result:` lines. Three runs is
      not enough at a 17% rate — that is the mistake this bug is correcting.
- [ ] `cargo test --lib shell::launch::` still reports **8 passed — 8, not 9.**
      This is a race fix and adds no tests.
- [ ] `shell::backend::` still **7**, `shell::pty::` **13**, `log` **12**,
      `screen` **11**, `host` **8**, `proto` **5**.
- [ ] All four gates pass.
