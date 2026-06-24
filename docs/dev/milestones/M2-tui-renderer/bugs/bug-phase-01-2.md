# Bug 2 on phase-01: ratatui path never enters raw mode — input box is non-functional in a real terminal (green-but-broken)

**Severity:** major
**Status:** open
**Filed:** 2026-06-24

The banned-construct fixes from bug-phase-01-1 are all correctly applied (no
`unsafe`, no `#[allow]`, no `.expect()` in `src/cli/commands/mod.rs`; build,
clippy, fmt, and all 763+27 tests pass). But a live end-to-end run under tmux —
the verification the phase doc explicitly requires and the executor deferred to
the principal engineer as "headless, N/A" — reveals that the `DAEMONEYE_RENDERER=ratatui`
path **never puts the terminal into raw mode**. The inline viewport draws its
border, but the terminal line discipline stays in canonical/cooked mode, so the
input box cannot accept or display per-keystroke editing. This is precisely the
"green-but-subtly-broken" failure mode the milestone exists to catch: the
hermetic `TestBackend` tests pass because `TestBackend` does not model a tty
line discipline.

## What's wrong

### No raw-mode entry anywhere on the ratatui path

`src/cli/render_ratatui.rs:36-48` — `RatatuiRendererStdout::new()`:

```rust
pub fn new(start_time: std::time::Instant) -> std::io::Result<Self> {
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
        },
    )?;
    Ok(Self { terminal, start_time })
}
```

`Terminal::with_options` does **not** enable raw mode — in ratatui/crossterm
that requires an explicit `crossterm::terminal::enable_raw_mode()` (the
official inline example calls it; `ratatui::init()` calls it for the
full-screen path). It is not called here, nor anywhere else on the ratatui
path:

- `src/cli/commands/mod.rs:243` deliberately skips it: *"Do NOT call
  set_raw_mode or setup_scroll_region."* and passes `old_termios: None`.
- A repo-wide grep confirms there is **no** `enable_raw_mode` call at all:
  ```
  $ grep -rnE 'enable_raw_mode|ratatui::init' src/
  (no matches)
  $ grep -rnE 'disable_raw_mode|try_restore' src/
  src/cli/render_ratatui.rs:86:        let _ = ratatui::try_restore();
  ```
  `restore()` calls `ratatui::try_restore()`, which *disables* raw mode — a
  no-op net effect, since it was never enabled.

The doc comment on `new()` is therefore **factually wrong** and actively
misleading (STANDARDS §2.3 — comments must not mis-describe the code):

`src/cli/render_ratatui.rs:33-35`:
```rust
/// Enters raw mode and constructs the terminal.  Callers must **not**
/// have called `set_raw_mode()` from `input.rs` before this — ratatui
/// manages raw mode internally and we avoid double-entering it.
```

It claims raw mode is entered and "managed internally"; neither is true.

### Observed live behavior (tmux capture-pane)

Launched `DAEMONEYE_RENDERER=ratatui ./target/debug/daemoneye chat` in a tmux
pane:

- In a *detached* session, after `new()` had already run, typing `hello world`
  echoed as **plain cooked-mode text** on its own line — proof the terminal is
  not in raw mode at startup (raw mode would suppress the echo):
  ```
  No foreground target set. Only background commands will run.

  hello world
  ```
- In an *attached* session the input box border renders, but typed characters
  (`abc`) never appear inside the box `│ … │` and are not delivered to the
  input editor — they are line-buffered/echoed by the kernel tty discipline,
  not read by `AsyncStdin` until Enter.

## What should happen

Phase-01 acceptance criterion: *"With `DAEMONEYE_RENDERER=ratatui`, the chat
client starts, **accepts typed input**, shows the input box and status bar in a
fixed bottom region, commits submitted user input and the AI's final answer
into terminal scrollback … and **exits cleanly restoring the terminal**."* The
End-to-end verification step requires typing and submitting a line and
confirming the box behaves. With the terminal in cooked mode, "accepts typed
input" into the live-edited box is not met, and clean restore is moot because
the mode was never changed.

The ratatui path must enter raw mode on startup and restore it on exit, with
the two renderers not fighting over terminal state (the existing constraint
that motivated `old_termios: None`).

## How to fix

1. Enter raw mode when the ratatui renderer starts — e.g. call
   `crossterm::terminal::enable_raw_mode()` in `RatatuiRendererStdout::new()`
   (and propagate the `io::Error`), or use the ratatui-provided init helper for
   the inline-viewport path. Confirm against the live ratatui/crossterm docs,
   as the lean spec intends.
2. Ensure `restore()` (already calling `ratatui::try_restore()`, which disables
   raw mode) is invoked on every exit path of the ratatui chat loop, including
   error/`Ctrl-C` returns, so the terminal is left in cooked mode.
3. Fix the `new()` doc comment so it describes what the code actually does (or
   delete it — STANDARDS §2.3 prefers no comment over a wrong one).
4. Keep the bug-phase-01-1 fixes intact (no `unsafe`, no `#[allow]`, no
   `.expect()`), and keep the legacy default path behavior-unchanged.

Note the `TestBackend` tests cannot catch this (no real tty); the executor
should reason about the live-terminal path, and the principal engineer
re-verifies under tmux on the next review.

## Verification

- [ ] `grep -rnE 'enable_raw_mode' src/` shows raw mode is entered on the
      ratatui path.
- [ ] Under tmux: `DAEMONEYE_RENDERER=ratatui daemoneye chat`, typed characters
      appear **inside** the input box (not echoed as cooked-mode text), a
      submitted line commits to scrollback above the fixed viewport, and on
      exit the terminal returns to cooked mode (shell echo works, no stuck raw
      state). Quote the `tmux capture-pane -p` output in the Update Log.
- [ ] The `new()` doc comment matches the code.
- [ ] `cargo fmt --all`, `cargo build` (zero warnings), `cargo clippy
      --all-targets --all-features -- -D warnings`, `cargo test` all pass.
- [ ] bug-phase-01-1 fixes still hold: no `unsafe`/`#[allow]`/`.expect()` in
      `src/cli/commands/mod.rs`.
