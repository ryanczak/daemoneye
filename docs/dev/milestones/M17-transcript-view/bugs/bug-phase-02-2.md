# Bug 2 on phase-02: the normal exit path disarms the guard, so `esc` never leaves the alternate screen

**Severity:** blocker
**Status:** open
**Filed:** 2026-08-19

## What's wrong

Round 2 moved the alternate-screen teardown into `AltScreenGuard`'s `Drop`
(`src/cli/viewer.rs:202-208`) — correct — and then **disabled it on the path
every user takes**.

The only executable `LeaveAlternateScreen` in the file is inside the guard's
teardown closure (`src/cli/viewer.rs:233`):

```rust
    let mut guard = AltScreenGuard::new(|| {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), Show);
        renderer.reanchor();
    });
```

`Drop` runs it only when armed (`viewer.rs:202-208`):

```rust
impl Drop for AltScreenGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            (self.teardown)();
        }
    }
}
```

And the normal exit path disarms it first (`viewer.rs:297-300`):

```rust
    guard.disarm();
    // Terminal drops before the guard (declared after it), so the fullscreen
    // buffer is cleared under the alternate screen before the screen is left.
    drop(terminal);
    Ok(())
```

`disarm()` sets `armed = false` (`viewer.rs:197-199`) and **nothing else leaves
the alternate screen**. So when the user presses `esc`, `q`, or `ctrl+o` to
close the viewer — the only ways to close it — the guard is disarmed, the
teardown never runs, and the chat client returns to its input loop with the
terminal still on the alternate screen, the cursor still hidden, and the inline
viewport never re-pinned.

The comment quoted above states the screen "is left" after the terminal drops.
It is not; that call was removed with the arming.

The behaviour is inverted relative to round 1. Round 1 worked on the normal path
and failed on the error path. Round 2 works on the error path and fails on the
normal path — the one taken 100% of the time.

The executor's own passing test documents the broken semantic:
`alt_screen_guard_disarmed_skips_teardown` (`viewer.rs:473-489`) asserts a
disarmed guard must **not** run teardown, and production disarms on the normal
path.

## What should happen

The teardown must run **exactly once on every exit path** — the `break` after
`esc`/`q`/`ctrl+o`, any `?` early return, and the final `Ok(())`. That is the
whole point of binding it to `Drop`: there is no path that skips it and no path
that runs it twice.

If ordering against the fullscreen `Terminal` matters (clearing the buffer
before leaving the screen), express it with scoping — let the `Terminal` live in
an inner scope that ends before the guard drops — not by disabling the guard.

## Root cause

`disarm()` exists to let a caller say "the screen was already left." No caller
ever leaves the screen, so the only thing `disarm()` can do in this file is skip
the teardown. It was introduced for ordering (`viewer.rs:298-299` comment) and
achieves ordering by removing the operation it was ordering.

The structural criteria added after bug-phase-02-1 did not catch this: they
assert a `Drop` impl exists and contains `LeaveAlternateScreen`, which is true
here. They never assert the teardown *runs* on the normal path. That is a
criterion-design defect on the architect's side, recorded as such.

## Not a defect (checked and cleared)

The round-2 error handler at `chat.rs:746` uses `eprintln!` while the terminal
is in raw mode. That was flagged at review and **cleared**: the same file
already does this inside the same loop at `chat.rs:370-372` (daemon
unreachable) and `chat.rs:572`. The executor matched the local convention.
Leave it alone.

## Definition of done

Each command below **fails against the current tree** (verified 2026-08-19) and
must pass:

- [ ] `grep -c "disarm" src/cli/viewer.rs` prints `0` — the guard has no
      disable path at all. (Currently `5`.)
- [ ] `grep -c "fn viewer_loop" src/cli/viewer.rs` prints at least `1` — the
      fallible body is factored into a helper so `run_transcript_viewer` is
      "enter, arm the guard, run the helper, return its result", and every exit
      path leaves through the same drop. (Currently `0`.)
- [ ] Test `alt_screen_guard_runs_teardown_on_normal_exit` passes: a guarded
      scope that returns **normally** (no error) must have run the teardown
      exactly once. Assert the count is `1`, not merely non-zero — a teardown
      that runs twice leaves the alternate screen twice and is also wrong.
      (Currently absent.)
- [ ] Test `alt_screen_guard_runs_teardown_on_drop` (from round 2) still passes
      for the early-return case.
- [ ] `awk '/impl Drop for/{f=1} f&&/LeaveAlternateScreen/{print "GUARD OK"; exit}' src/cli/viewer.rs`
      still prints `GUARD OK`, and
      `grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs`
      still prints nothing and exits 1.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
