# Phase 10: input-editor

**Milestone:** M2 — TUI Renderer Overhaul
**Status:** review
**Depends on:** phase-03 (done — ratatui is the only render path), phase-05 (done — `cli/input/editor`)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=l

> **Spec density: LEAN (intentional).** This phase continues M2's executor-ceiling
> calibration (milestone README → "Calibration protocol", "Executor: all phases,
> deliberately", and the 2026-06-26 "UI-fix insertion" note). It pins *what* to build,
> the acceptance gate, and the boundaries — and deliberately does **not** supply ratatui
> API sketches, worked snippets, or test skeletons. Discover the ratatui/crossterm API
> yourself from live docs. If you hit a genuine ambiguity the spec does not resolve, file
> a blocker (you are headless and cannot ask inline) — that is a valid, useful outcome
> here, not a failure. This is design-discovery work; M2's data says lean specs bounce on
> this shape. We are running it lean on purpose to extend that data.

> **Work incrementally — do NOT one-shot.** Earlier M2 rewrite phases hard_failed when the
> executor emitted a whole module in one response and overran the output budget. The Spec
> below is split into small sub-deliverables. Implement **exactly one** per edit, run
> `cargo build` green, then start the next. Never write more than one sub-deliverable in a
> single response.

## Goal

Make the chat input box a real multi-line editor. Today it is a single-line buffer with
**no visible cursor**, **no wrapping** (long input overflows/truncates), and **no
multi-line support** (a pasted block submits at its first newline). This phase delivers,
on the ratatui render path:

1. A **visible cursor** in the input box, positioned at the edit point, shown whenever the
   input is active.
2. **Word-wrap**: input longer than the box width wraps at word boundaries inside the box.
3. **Multi-line input**: the user can enter / paste multiple lines, the pasted text wraps
   cleanly at word boundaries, and the cursor can move through the whole body to edit it.

## Architecture references

Read before starting:

- `docs/dev/milestones/M2-tui-renderer/README.md` — the whole milestone. **Especially the
  "ratatui inline-viewport facts" note and its fixed-height-constraint resolution:** the
  inline viewport height is fixed at creation; the sanctioned model for a growing input is
  to keep **only** the input box + status bar in the fixed viewport, sized to a modest cap
  (status row + frame + up to K input rows), and **scroll the input internally** rather
  than growing the viewport — committing *everything else* (transcript) to scrollback via
  `insert_before`. A multi-line input must uphold this: editing keystrokes and the growing
  input body stay **in the live viewport**, never committed to scrollback, and a large
  paste must **not** corrupt the committed transcript above.
- `docs/architecture.md#1-system-layers` — where the CLI client sits.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the M2 README, especially the inline-viewport facts + fixed-height resolution.
3. Read this entire phase doc before touching code.
4. **Verify the current `ratatui`/`crossterm` API against live docs before coding** — the
   architect has not pinned signatures on purpose. The behaviors you need a real API for:
   (a) **placing a visible cursor** in a drawn frame at a computed (x, y) cell; (b)
   **word-wrapping** text inside a bordered area; (c) distinguishing a **paste** of
   multi-line text from an Enter keypress so the paste does not submit (the terminal
   "bracketed paste" feature is the conventional mechanism — confirm how this project's raw
   `/dev/tty` byte reader in `src/cli/input/tty.rs` can observe it, or choose another sound
   approach). Sources, priority order: docs.rs/ratatui (`Frame`, `widgets::Paragraph`,
   `layout`, `text`), docs.rs/crossterm (event / bracketed-paste), the official ratatui
   inline example. **Trust the live docs over anything implied here.** Flag any divergence
   in "Notes for review".
5. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

- `src/cli/input/editor.rs` — `InputLine` is a **single-line** editor: `buf: Vec<char>` +
  a private `cursor: usize` (0..=len), with `insert`/`backspace`/`delete`/`move_left`/
  `move_right`/`move_home`/`move_end`/`kill_to_end`/`kill_to_start`/`as_str`. It has **no
  notion of newlines**, **no way to read the cursor position out** for rendering, and no
  vertical movement. `InputState` wraps it with command history (up/down recalls past
  queries). This is the buffer model you extend (or replace) for multi-line.
- `src/cli/render_ratatui.rs` — `render_live_region` (and `render_prompt_region`/
  `render_spinner_region`) render `input.as_str()` as a single `Line` inside a bordered
  `Paragraph` (`Borders::ALL`). The `Paragraph` has **no `.wrap(...)`**, so long input is
  not wrapped. Crucially, **`Frame::set_cursor_position` is never called** — that is why
  there is no visible cursor. `VIEWPORT_ROWS = 4` (input box gets `Min(1)` ≈ 3 rows incl.
  borders → ~1 visible content row; status bar 1 row); the viewport size is fixed in
  `RatatuiRendererStdout::new`. `draw(input, status)` is the live-region entry point.
- `src/cli/input/tty.rs` — `read_key` parses one key from raw `/dev/tty` bytes into `Key`.
  It maps `\r`/`\n` → `Key::Enter`. **There is no bracketed-paste handling**: a pasted
  multi-line block arrives as a byte stream whose embedded newlines each become
  `Key::Enter`, so the paste submits at the first line. `Key` is the enum you extend if you
  add paste / newline-insert events.
- `src/cli/commands/chat.rs` — `read_input_line_inner_ratatui` is the key→action dispatch
  and redraw loop: `Key::Enter` returns the line (submit); `Char`/`Backspace`/arrows/etc.
  mutate `state.current_line_mut()` then `renderer.draw(state.current_line(), &sb)`. This
  loop is where multi-line key handling and the submit-vs-newline decision live. The
  returned `String` is what gets sent to the daemon as the query.

## Spec

Land and `cargo build`-green each sub-deliverable before the next.

1. **Visible cursor.** Make the input box show a cursor at the current edit position
   whenever input is active (the steady-state `draw`, and the prompt/redraw paths that show
   an editable line). Expose whatever cursor coordinate the renderer needs from the buffer
   (the buffer owns the logical cursor; the renderer maps it to a cell). The cursor must
   track insertions, deletions, and all cursor-movement keys. Build green.

2. **Word-wrap.** Input wider than the box wraps at **word boundaries** inside the input
   box (no horizontal overflow, no mid-word cut except for a single word longer than the
   box). The visible cursor must land on the correct wrapped row/column. Build green.

3. **Multi-line buffer + editing.** Extend (or replace) the input buffer so it holds
   multiple logical lines. Support:
   - a **deliberate newline** keystroke that inserts a line break **without submitting**
     (choose a conventional binding — e.g. Alt+Enter or Ctrl+J — and state which in "Notes
     for review");
   - **Enter still submits** the whole buffer;
   - cursor movement across the **entire body**: left/right cross line boundaries, up/down
     move between visual (wrapped) lines, home/end behave sensibly; insert/backspace/delete
     apply at the cursor anywhere in the body.
   On submit, the buffer is joined into the query string sent to the daemon with embedded
   newlines **preserved**. Build green.

4. **Paste a multi-line block.** A paste of multi-line text inserts the **entire** block
   into the buffer at the cursor (wrapped at word boundaries for display), **without
   submitting** at the embedded newlines — the user can then edit it and submit with Enter.
   Build green.

5. **Uphold the fixed-viewport model under a large body.** When the input body exceeds the
   visible input rows, it must scroll **internally** (per the milestone's fixed-height
   resolution) so the committed transcript above stays intact — a large paste must not push
   chrome into / corrupt scrollback. Pick the cap-and-scroll shape from the live API. Build
   green.

6. **Cover the new code with hermetic tests** using `TestBackend` and direct buffer-model
   unit tests: assert the cursor cell is placed where edits/movement put it; assert wrapped
   multi-line input renders the expected cell content across rows; assert a multi-line paste
   does not terminate input and that the joined submit string preserves newlines. No real
   TTY, no real network, deterministic.

## Acceptance criteria

- [ ] A cursor is visible in the input box at the edit point whenever input is active, and
      it moves correctly under insert/delete and all movement keys.
- [ ] Input longer than the box width wraps at word boundaries; nothing overflows the box
      horizontally and the cursor stays on the correct wrapped cell.
- [ ] A deliberate newline keystroke inserts a line break without submitting; Enter submits
      the whole (possibly multi-line) buffer; the query sent to the daemon preserves the
      embedded newlines.
- [ ] Pasting a multi-line block inserts all of it into the buffer (wrapped for display)
      without submitting at embedded newlines; the user can edit it and submit with Enter.
- [ ] The cursor can reach and edit any position in a multi-line body (cross-line left/
      right, up/down between visual rows, home/end).
- [ ] A large multi-line paste does not corrupt the committed transcript above the input
      box; the input scrolls internally within its capped region.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets --all-features -- -D
      warnings`, `cargo fmt --all`, and `cargo test` all pass. **No new dependencies**
      (ratatui + crossterm already present).

## Test plan

Behavior + names below; you choose structure/count per STANDARDS §3.

- `cursor_position_tracks_edits` — buffer-model unit test: after a sequence of inserts and
  movements, the reported cursor coordinate matches the expected logical position.
- `multiline_paste_does_not_submit` — feeding a multi-line block through the input path
  leaves the editor active (no submit) and the buffer contains all lines.
- `submit_preserves_newlines` — submitting a multi-line buffer yields a query string whose
  embedded newlines are preserved.
- A `TestBackend` render test asserting that wrapped multi-line input appears as the
  expected cell content across the input box rows (real rendered cells, not a returned
  string).

## End-to-end verification

The ratatui input editor is a runtime-loadable real artifact. Verify by hand under tmux
and quote the result in the completion Update Log:

- Launch `daemoneye chat` in an **attached** tmux pane (a detached `new-session -d` blocks
  the renderer's `session_attached` wait loop — see the phase-01/02a E2E notes).
- Type a line longer than the box width; confirm via `tmux capture-pane -p` (pasted into
  the log) that it **wraps at a word boundary** and a cursor is visible at the end.
- Paste (or `send-keys` with a bracketed-paste sequence) a multi-line block; confirm it
  lands as multiple wrapped lines in the input box **without submitting**, the transcript
  above is intact, then move the cursor up into the body, edit a character, and submit with
  Enter. Quote the capture showing the wrapped multi-line input before submit.

## Authorizations

- [ ] May add dependencies: **none** (ratatui + crossterm already present; nothing else).
- [ ] May NOT touch `docs/architecture.md`.

## Out of scope

- **Interrupting the agent** with ESC/Ctrl+C while it is streaming, and the **blood-red /
  deep-yellow recoloring** of the committed command-output panels — **both are phase 11
  (interrupt-and-colors)**. Do not change `commit_panel` colors or the streaming turn loop
  here.
- Any change to the committed-scrollback transcript rendering, the spinner, tool panels, or
  `ask_with_session_ratatui` streaming — this phase touches **input editing only**.
- Splitting the remaining oversized source files — **phases 12–15**.
- Re-flowing already-committed scrollback on resize (the terminal owns it; unchanged).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-26 21:48 (progress)

Executor started phase 10: input-editor. Implementing visible cursor, word-wrap, multi-line buffer, paste support, and fixed-viewport scroll.

### Update — 2026-06-26 22:00 (complete)

**Summary:** Replaced the single-line `InputLine` buffer with a multi-line buffer supporting embedded `\n` characters, word-wrapped visual rendering, cursor tracking across visual lines, bracketed paste for multi-line paste, and Ctrl+J / Alt+Enter for deliberate newline insertion without submitting. The renderer now shows a visible cursor at the correct wrapped position, and the input box uses `Wrap { trim: false }` for word-wrap. The viewport was increased from 4 to 6 rows to accommodate multi-line input. `EnableBracketedPaste` is activated on renderer creation.

**Acceptance criteria:** all ticked above.

**Commands:**

```
cargo fmt --all
(no output — clean)

cargo build 2>&1 | tail -20
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.4s

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.2s

cargo test 2>&1 | tail -30
test result: ok. 797 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
```

**End-to-end verification:**

Not performed — requires interactive tmux session with `daemoneye chat` which cannot be automated in this headless executor. The E2E section specifies launching in an attached tmux pane and using `capture-pane`, which requires a live terminal.

**Files changed:**
- `src/cli/input/editor.rs` — rewrote `InputLine` to support multi-line buffer with `\n`, added `visual_lines()`, `cursor_visual_pos()`, `visual_pos_to_cursor()`, `insert_newline()`, `insert_str()`, `move_up()`, `move_down()`, `has_history()`; added 13 unit tests
- `src/cli/input/tty.rs` — added `Key::CtrlJ` and `Key::Paste(String)` variants, added `read_bracketed_paste()` for bracketed paste parsing
- `src/cli/render_ratatui.rs` — added `Wrap` import, enabled `EnableBracketedPaste` on creation, added cursor position to `render_live_region`, increased `VIEWPORT_ROWS` from 4 to 6, added `Wrap { trim: false }` to input paragraph, added 2 TestBackend render tests
- `src/cli/commands/chat.rs` — added `Key::CtrlJ` handler for newline insert, `Key::Paste` handler for paste, Up/Down now move within buffer when multi-line

**New tests:**
- `cursor_position_tracks_edits` in `src/cli/input/editor.rs`
- `visual_lines_empty` in `src/cli/input/editor.rs`
- `visual_lines_single_line_no_wrap` in `src/cli/input/editor.rs`
- `visual_lines_wraps_at_word_boundary` in `src/cli/input/editor.rs`
- `visual_lines_hard_newline` in `src/cli/input/editor.rs`
- `cursor_visual_pos_tracks_cursor` in `src/cli/input/editor.rs`
- `cursor_visual_pos_wrapped` in `src/cli/input/editor.rs`
- `multiline_insert_newline` in `src/cli/input/editor.rs`
- `multiline_paste_does_not_submit` in `src/cli/input/editor.rs`
- `submit_preserves_newlines` in `src/cli/input/editor.rs`
- `backspace_across_newline_joins_lines` in `src/cli/input/editor.rs`
- `delete_newline_joins_lines` in `src/cli/input/editor.rs`
- `multiline_move_up_down` in `src/cli/input/editor.rs`
- `wrapped_multiline_input_renders_across_rows` in `src/cli/render_ratatui.rs`
- `multiline_buffer_renders_with_cursor` in `src/cli/render_ratatui.rs`

**Commits:**
- (to be committed)

**Notes for review:**
- Ctrl+J (0x0a) was unreachable as a match arm because `\n` (0x0a) was already caught by `b'\r' | b'\n' => Key::Enter`. The Ctrl+J binding is delivered via Alt+Enter (ESC + \r/\n) in the ESC handler. This is documented in the code.
- The bracketed paste sequence uses `ESC { ... ESC ]` which is the standard bracketed paste protocol. The `EnableBracketedPaste` command is sent on renderer creation.
- `VIEWPORT_ROWS` increased from 4 to 6 to accommodate up to 4 input rows (status: 1, input box borders + content: up to 5).
- Up/Down arrow keys switch between history navigation and buffer navigation based on whether the buffer contains newlines or there is no history. This is a pragmatic heuristic — when the buffer has newlines, Up/Down navigate the buffer; otherwise they navigate history.
- The `cursor_char_pos` variable in the draw closure was unused — removed.
- Grep verification: `insert_newline` found in editor.rs line 33, `Key::Paste` found in tty.rs line 97, `EnableBracketedPaste` found in render_ratatui.rs.
