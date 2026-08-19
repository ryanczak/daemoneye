# Bug 1 on phase-02: a viewer error strands the user on the alternate screen and kills the chat session

**Severity:** major
**Status:** resolved (round 2, `ea7ebe4`)
**Filed:** 2026-08-18

## What's wrong

`run_transcript_viewer` (`src/cli/viewer.rs:183`) enters the alternate screen at
line 195 and leaves it at line 260:

```rust
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
```

```rust
    drop(terminal);
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    let _ = execute!(std::io::stdout(), Show);
    renderer.reanchor();
```

Between those two points there are seven fallible calls using `?` —
`terminal.size()?` and `terminal.draw(...)?` in the initial draw, in the
`sigwinch` arm, and in the key loop. **Any one of them returns early, so
`LeaveAlternateScreen` and `reanchor()` never run.**

It does not stop there. The call site added in `src/cli/commands/chat.rs:737-739`
propagates the error out of the input loop:

```rust
                    Key::CtrlO => {
                        crate::cli::viewer::run_transcript_viewer(stdin, sigwinch, renderer, transcript)
                            .await?;
```

`read_input_line_inner_ratatui` is awaited with `?` at `chat.rs:413`, and
`renderer.restore()` sits **after** the loop at `chat.rs:592`. So the error
unwinds out of `run_chat_ratatui` without restoring anything.

Observed end state on any viewer I/O error: the chat process exits with the
terminal still on the alternate screen **and** still in raw mode — no prompt,
no echo. The user's pane needs `reset` or closing.

## What should happen

Two independent properties, per the phase spec's § Spec task 4 step 5 ("On
break: drop the fullscreen terminal, then `LeaveAlternateScreen`, then
`renderer.reanchor()`"):

1. **Leaving the alternate screen and re-pinning the inline viewport must
   happen on every exit path**, including an error return — not only on the
   `break` path.
2. **A viewer failure must not terminate the chat session.** `ctrl+o` opening a
   pager is not a fatal operation; if it fails, the session continues.

The codebase already has the idiom for property 1 — an RAII guard whose `Drop`
performs the teardown. `src/daemon/executor/foreground.rs:50-80` — `FgHookGuard` owns the tmux hooks
it installed and tears them down in `Drop`:

```rust
struct FgHookGuard {
    target: String,
    hooks: Vec<String>,
    monitor_silence: bool,
}

impl Drop for FgHookGuard {
    fn drop(&mut self) {
        for hook in &self.hooks {
            let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
                "set-hook",
                // …
```

Note that its teardown uses `let _ =` rather than `?` — a `Drop` cannot
propagate, and cleanup that cannot run is worse than cleanup that cannot report.

The guard runs its cleanup whether the scope exits normally or early, which is
exactly the property the `?` operators break here.

## Root cause

Cleanup is written as **straight-line statements at the end of the happy path**
(`viewer.rs:258-262`) rather than bound to the scope that owns the alternate
screen. `?` is the project's default propagation operator per
`docs/dev/STANDARDS.md` §2.1 and is correct here — what is missing is that the
alternate screen is a *resource* whose release must not be a statement the
control flow can skip.

The second half is at the call site: `.await?` at `chat.rs:739` promotes a
non-fatal pager failure into session termination, and `renderer.restore()` at
`chat.rs:592` is unreachable from that path.

## Definition of done

Each command below **fails against the current tree** (verified 2026-08-18) and
must pass:

- [ ] `awk '/impl Drop for/{f=1} f&&/LeaveAlternateScreen/{print "GUARD OK"; exit}' src/cli/viewer.rs`
      prints `GUARD OK` — the alternate-screen exit runs from a `Drop`
      implementation, not from a statement at the end of the happy path.
      (Currently prints nothing.)
- [ ] `grep -c "impl Drop" src/cli/viewer.rs` prints `1`. (Currently `0`.)
- [ ] `grep -A2 "run_transcript_viewer" src/cli/commands/chat.rs | grep -c "await?"`
      prints `0` — a viewer error is handled at the call site (logged and/or
      shown), never propagated out of the input loop. (Currently `1`.)
- [ ] `renderer.reanchor()` still runs after the viewer closes on the normal
      path: `grep -c "reanchor()" src/cli/viewer.rs` prints at least 1.
- [ ] The existing negative criterion still holds:
      `grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs`
      prints nothing and exits 1.
- [ ] All eight `cli::viewer` tests still pass, plus a new test
      `alt_screen_guard_runs_teardown_on_drop` asserting the guard's `Drop`
      performs its teardown exactly once when the guarded scope exits early.
      Structure the guard so this is assertable without a real terminal (for
      example, by having the guard call an injectable teardown action that the
      test can count). The teardown action used in production must be the one
      that leaves the alternate screen.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
