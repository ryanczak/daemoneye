# Bug 1 on phase-01: banned constructs in the ratatui wiring (new `unsafe`, lint-silencing `#[allow]`, production `expect`) — one is a latent terminal-corruption bug

**Severity:** major
**Status:** open
**Filed:** 2026-06-24

The phase is functionally plausible and builds green, but the wiring in
`src/cli/commands/mod.rs` reaches for three constructs the Definition of Done
(STANDARDS §1) explicitly bans. They share a single root cause — the new
ratatui code path was bolted onto the existing `TerminalCtx` / chat-loop
signatures instead of being given a shape that fits it — and a single fix
(adopt the codebase's own context-struct idiom + make the unused termios
optional). One of them is additionally a real latent bug, not just a style
violation.

## What's wrong

### 1a — New `unsafe { std::mem::zeroed() }` ×3 (banned; also a latent bug)

`src/cli/commands/mod.rs:261`, `:576`, `:806`:

```rust
old_termios: unsafe { std::mem::zeroed() }, // unused for ratatui
```

STANDARDS §1: *"No `unsafe` blocks. (If you think you need one, stop and
report a blocker — `unsafe` requires principal-engineer review.)"* Three new
`unsafe` blocks were introduced with no blocker filed.

These are **not** harmless. The zeroed `libc::termios` is carried in
`TerminalCtx.old_termios` (`mod.rs:414`) and `StreamCtx.old_termios`
(`stream.rs:116`), and flows into the tool-approval UI, which restores it to
the **real** terminal:

- `src/cli/commands/approval_ui.rs:194` (and 295, 336, 397, 459, 520, 639,
  716): `restore_termios(old_termios);`
- `src/cli/input.rs:284-288`:
  ```rust
  pub fn restore_termios(old: libc::termios) {
      unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &old); }
  }
  ```

So the first time a tool call is approved in the `DAEMONEYE_RENDERER=ratatui`
path, `tcsetattr` applies a **zeroed** termios to stdin — `c_lflag == 0` wipes
canonical mode, echo, and signal generation, leaving the user's terminal in a
broken mode. The `// unused for ratatui` comment is wrong: it is used.

### 1b — New `#[allow(clippy::too_many_arguments)]` ×3 (lint-silencing shim, banned)

`src/cli/commands/mod.rs:422`, `:466`, `:843`. STANDARDS §1: *"No
`#[allow(...)]`, `#[ignore]`, or lint-silencing shims to mask diagnostics."*

The new `run_chat_ratatui` and `read_input_line_inner_ratatui` take 12–15
positional parameters, which trips `clippy::too_many_arguments`. The executor
silenced the lint rather than fixing the cause — even though the established
idiom for exactly this is **already in the same file** and used by the legacy
path: `InputHandles`, `TerminalCtx`, `TmuxCtx`, `StreamCtx`, `TokenCtx`,
`StreamResizeDims`. The new code should group its parameters the same way.

### 1c — New `.expect()` in a production path

`src/cli/commands/mod.rs:449`:

```rust
let mut renderer = renderer.expect("ratatui renderer required");
```

STANDARDS §1 forbids `.expect()` in production paths. This exists only because
`renderer: Option<RatatuiRendererStdout>` and `renderer_mode: RendererMode`
are threaded as two uncoupled parameters, so the "renderer is `Some` iff mode
is `Ratatui`" invariant is not type-enforced and has to be re-asserted at
runtime. Folding the renderer into the ratatui-path context struct (1b)
removes the `Option` and the `expect` together.

## What should happen

No new `unsafe`, no `#[allow]` lint-silencing, no production `expect` — per
STANDARDS §1. The ratatui path should follow the file's own context-struct
idiom, and the `old_termios` field should not have to be fabricated for a path
that does not own a saved termios.

## How to fix

1. **Remove the zeroed-termios `unsafe` (1a).** Make the saved termios optional
   for the ratatui path rather than fabricating one. Options, in rough order of
   cleanliness:
   - Change `TerminalCtx.old_termios` / `StreamCtx.old_termios` to
     `Option<libc::termios>` and have `restore_termios` callers skip restoration
     when `None` (the ratatui renderer owns raw-mode restore via
     `ratatui::try_restore()`), **or**
   - give the ratatui path its own context struct that simply does not carry a
     termios.
   Either way: zero new `unsafe`, and no zeroed termios can ever reach
   `tcsetattr`.

2. **Remove the three `#[allow(clippy::too_many_arguments)]` (1b).** Consolidate
   the parameters of `run_chat_ratatui` and `read_input_line_inner_ratatui` into
   context struct(s), mirroring the existing `InputHandles` / `TerminalCtx` /
   `TmuxCtx` / `StreamCtx` pattern in this file, until clippy is satisfied
   without the shim.

3. **Remove the `.expect()` (1c)** by threading the renderer through the new
   ratatui context struct so the `Option` is unnecessary, or otherwise
   restructuring so the renderer's presence is type-guaranteed on the ratatui
   path.

Keep the legacy path behavior-unchanged and keep the new code building green at
each step (work incrementally per the existing "Notes for executor" guidance).

## Verification

- [ ] `grep -nE 'unsafe' src/cli/commands/mod.rs` returns nothing (no new
      `unsafe` in this file).
- [ ] `grep -nE '#\[allow' src/cli/commands/mod.rs` returns nothing.
- [ ] `grep -nE '\.expect\(' src/cli/commands/mod.rs` returns nothing in the new
      ratatui code path.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes with no
      `#[allow]` shims.
- [ ] `cargo fmt --all`, `cargo build` (zero warnings), `cargo test` all pass.
- [ ] Manually confirmed (or reasoned from the code) that approving a tool call
      under `DAEMONEYE_RENDERER=ratatui` no longer applies a zeroed termios to
      stdin.
