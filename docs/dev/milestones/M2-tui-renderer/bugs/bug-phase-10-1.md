# Bug 1 on phase-10: three core input behaviors are green-but-inert on a real terminal

**Severity:** blocker
**Status:** resolved
**Filed:** 2026-06-26

## What's wrong

The build is green and every new unit test passes, but **three of the phase's core
acceptance criteria are non-functional on a real terminal** because every test
exercises the buffer model (`InputLine`) directly and bypasses the two seams that
actually carry the behavior: the `/dev/tty` key parser (`cli/input/tty.rs`) and the
ratatui render path (`cli/render_ratatui.rs`). This is precisely the "green-but-inert"
failure mode M2 exists to catch.

### Defect 1 — deliberate newline keystroke is unreachable (AC3)

`Key::CtrlJ` is declared (`src/cli/input/tty.rs:96`) and handled
(`src/cli/commands/chat.rs:608`), but **nothing in `read_key` ever constructs it**:

- `src/cli/input/tty.rs:153` maps `b'\r' | b'\n' => Key::Enter`. Ctrl+J sends `0x0a`
  (`\n`), so it is swallowed as **Enter and submits**.
- The ESC handler (`tty.rs:161-202`) has arms only for `[`, `O`, and `{`. Alt+Enter
  sends `ESC \r`; the `\r` falls through to `_ => Key::Char('\x1b')` (bare escape), so
  Alt+Enter does **not** produce `Key::CtrlJ` either.

The executor's own "Notes for review" claim "The Ctrl+J binding is delivered via
Alt+Enter (ESC + \r/\n) in the ESC handler" — but no such arm exists in the code. The
`Key::CtrlJ` match arm in `chat.rs:608` is dead. (It does not warn because `Key` is a
`pub enum` in a lib crate, so the never-constructed variant escapes `dead_code`.)
**Result:** there is no keystroke that inserts a newline without submitting.

### Defect 2 — bracketed-paste parser keys on a fabricated protocol (AC4)

`EnableBracketedPaste` is activated (`src/cli/render_ratatui.rs:150-152`), so the
terminal wraps a paste in the real xterm sequence `ESC [ 2 0 0 ~` … `ESC [ 2 0 1 ~`.
But the parser keys paste-**start** on `ESC {` (`src/cli/input/tty.rs:196`), which no
terminal sends, and keys paste-**end** on `ESC ]` (`tty.rs:245`, which is actually
OSC-start). On a real terminal a paste begins with `ESC [`, matched at `tty.rs:163`;
the next byte `2` then falls through to `_ => Key::Char('\x1b')` at `tty.rs:186`, so the
paste's lead bytes are dropped and the body streams in as ordinary keys — **the
embedded newlines become `Key::Enter` and the paste submits at the first line**, the
exact bug this phase exists to fix.

The executor's note "ESC { ... ESC ] which is the standard bracketed paste protocol" is
factually wrong; the standard is `ESC[200~` / `ESC[201~`. The unit test
`multiline_paste_does_not_submit` (`editor.rs:483`) calls `insert_str` directly and
never touches `read_key`, so it passes while the real paste path is broken.

### Defect 3 — internal scroll under a large body is not implemented (AC6)

Spec item 5 / AC6 require the input to **scroll internally** within its capped region
when the body exceeds the visible rows, so the cursor stays visible and the committed
transcript above stays intact. The input `Paragraph`
(`src/cli/render_ratatui.rs:390-393` and `461-464`) has `.wrap(...)` but **no
`.scroll((offset, 0))`**, and nothing computes a scroll offset from the cursor row.
`VIEWPORT_ROWS` was merely raised 4→6 (`render_ratatui.rs:119`), giving ~3 fixed
content rows. When the body exceeds ~3 visual rows the lower lines are clipped and the
cursor `y` is clamped at `row.min(height-2)` (`render_ratatui.rs:398`), **detaching the
cursor from the edit position**. The load-bearing "scroll internally" half of the
milestone's fixed-height resolution is missing. (The scrollback-corruption half does
hold — ratatui's inline viewport owns that — so a large paste won't corrupt the
transcript; it just can't be seen or edited past row 3.)

### Defect 4 (minor, supporting) — two divergent wrap algorithms (AC2)

The cursor cell is computed by the hand-rolled `visual_lines` / `cursor_visual_pos`
(`src/cli/input/editor.rs:167, 241`), which **collapses runs of whitespace to a single
space** (`editor.rs:191-210`), drops leading whitespace, and **never splits a word
longer than the box** (`editor.rs:218-228`). The text is drawn by
`Paragraph::wrap(Wrap { trim: false })` (`render_ratatui.rs:392`), which preserves
whitespace and breaks over-long words at the width. The two disagree on multi-space,
leading-space, and over-long-word input, so the cursor lands on the wrong cell there —
violating AC2 ("the cursor stays on the correct wrapped cell"). It happens to agree for
ordinary single-spaced text under the box width, which is why the render tests pass.

## What should happen

Per the phase Spec and Acceptance criteria, on a **real terminal**:

- A deliberate-newline keystroke (Ctrl+J or Alt+Enter — state which) inserts a line
  break **without submitting**; Enter still submits (AC3).
- Pasting a multi-line block inserts the whole block without submitting at embedded
  newlines (AC4).
- When the body exceeds the visible input rows it scrolls internally so the cursor
  stays visible and the transcript above is intact (AC6).
- The visible cursor lands on the cell ratatui actually drew the character in, including
  multi-space / leading-space / over-long-word input (AC2).

## How to fix

1. **Newline keystroke (Defect 1).** Pick a binding that actually arrives and route it
   to `Key::CtrlJ` (or rename). E.g. handle `ESC` followed by `\r`/`\n` as the
   Alt+Enter newline in the ESC match (`tty.rs:161`), and/or stop mapping `\n` to
   `Enter` and give `0x0a` its own `Key::CtrlJ` arm while `\r` remains submit. State the
   chosen binding in Notes for review.
2. **Paste protocol (Defect 2).** Parse the real sequence: after `ESC [`, detect the
   `2 0 0 ~` start and read until `ESC [ 2 0 1 ~`. Keep `EnableBracketedPaste`.
3. **Internal scroll (Defect 3).** Compute a vertical scroll offset from the cursor's
   visual row so the cursor row stays within the visible content rows, and pass it to
   `Paragraph::scroll((offset, 0))`. Clamp the cursor `y` to the same window.
4. **Wrap source of truth (Defect 4).** Make the cursor cell derive from the **same**
   wrapping ratatui renders — e.g. pre-wrap in the buffer and feed ratatui pre-split
   lines with `Paragraph` wrap off, or otherwise unify so one algorithm governs both.

Add tests that exercise the **real seams**, not just `insert_str`:

- Feed the raw byte stream `\x1b[200~line1\nline2\x1b[201~` through `read_key` and assert
  a single `Key::Paste("line1\nline2")` (and that no `Key::Enter` is emitted mid-paste).
- Feed the newline keystroke's real bytes through `read_key` and assert `Key::CtrlJ`.
- A `TestBackend` test with a body taller than the content area asserting the cursor row
  is within the visible region and the line under the cursor is rendered.

## Verification

- [ ] `read_key` over `\x1b[200~a\nb\x1b[201~` yields one `Key::Paste("a\nb")`; no submit.
- [ ] `read_key` over the chosen newline keystroke's bytes yields `Key::CtrlJ`.
- [ ] A 10-line body in a ~3-row content box renders the cursor row visibly (scroll test).
- [ ] Cursor cell matches rendered cell for input with double spaces and an over-long word.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features -- -D
      warnings`, `cargo test` all green.
- [ ] End-to-end (architect/PE — the executor is headless and cannot drive interactive
      tmux): `send-keys` a bracketed-paste block, `capture-pane -p` shows multi-line
      wrapped input without submit; a newline keystroke adds a line; a tall body scrolls.
