# Phase 04: Cursor alignment — one wrapper, correct clamp, one width

**Milestone:** M13 — Chat UX Polish
**Status:** todo
**Depends on:** phase-03
**Estimated diff:** ~220 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

The chat input box draws its cursor a space (or more) away from the character
being edited. Three independent defects compound: the cursor position and the
rendered text use two *different* wrapping algorithms; the border clamp can
place the cursor on the border column/row; and cursor-movement key handling
wraps against a different width than the renderer draws with. This phase makes
`InputLine::visual_lines` the single authority for all three.

## Architecture references

Read before starting:

- `docs/dev/milestones/M13-chat-ux/README.md` § "Derived code facts" issue 2 —
  the three-defect inventory.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(All line numbers and grep counts verified 2026-08-10 against the
post-phase-03 tree.)

- **Defect 1 — two wrappers.** `draw` (`src/cli/render_ratatui.rs:238-264`)
  builds the input text as *logical* lines and computes the cursor from the
  hand-written word-wrap:

  ```rust
  let s = input.as_str();
  let input_text: ratatui::text::Text<'_> =
      s.split('\n').map(|l| Line::from(Span::raw(l))).collect();
  ...
      let content_width = area.width.saturating_sub(2) as usize;
      let (vis_row, vis_col) = input.cursor_visual_pos(content_width);
  ```

  while `render_live_region` (`:466`) renders that text through **ratatui's**
  wrapper: `.wrap(Wrap { trim: false })` (`:503`). `Paragraph::wrap` and
  `InputLine::visual_lines` (`src/cli/input/editor.rs:184`) disagree on
  whitespace and word-boundary handling, so cursor and glyph diverge on any
  wrapped line.
- **Defect 2 — clamp lands on the border.** `render_live_region:508-514`:

  ```rust
  if let Some((col, row)) = cursor_pos {
      let visible_row = (row as usize).saturating_sub(scroll_offset as usize);
      let x = content_area.x + 1 + col.min(content_area.width.saturating_sub(2));
      let y =
          content_area.y + 1 + (visible_row as u16).min(content_area.height.saturating_sub(2));
      frame.set_cursor_position((x, y));
  }
  ```

  Inner content spans columns `x+1 ..= x+width-2`; the clamp `min(width-2)`
  yields `x + width - 1` — the right-border column. Same for `y` with
  `height-2` → the bottom-border row. The correct clamp is `saturating_sub(3)`.
- **Defect 3 — two widths.** Key handling in `src/cli/commands/chat.rs`
  (`Key::Up` / `Key::Down` arms, two
  `let content_width = chat_width.saturating_sub(2);` sites — grep count
  verified: 2) wraps against `chat_width`, which comes from
  `terminal_width()` — deliberately `ws_col - 1` (`src/cli/render.rs:190-192`)
  — or a tmux pane query, while the renderer wraps against the real
  `area.width - 2`. Off by ≥ 1, so Up/Down cross wrapped rows at different
  points than the display shows. `renderer` is in scope in both arms (the
  sibling `Key::FocusGained` arm calls `renderer.reanchor()`, and the
  SIGWINCH arm calls `renderer.draw(...)`).
- `render_prompt_region` (`:589-591`) also uses `Paragraph::wrap` — that path
  sets no cursor at all and is **out of scope**; after this phase the file has
  exactly **one** `.wrap(Wrap { trim: false })` left (currently 2).
- `cursor_visual_pos(width) -> (row, col)` (`editor.rs:301`) is
  `visual_lines`' cursor-side twin — both are already unit-tested together
  (`editor.rs` `visual_lines_*` / `cursor_visual_pos_*` suites), which is why
  they can be the single authority.
- ratatui's `TestBackend` records the cursor set by
  `frame.set_cursor_position`: `Backend::get_cursor_position(&mut self) ->
  Result<Position>` (verified in
  `ratatui-core-0.1.2/src/backend/test.rs:272`). Existing tests access
  `renderer.terminal.backend()` directly (same module), e.g.
  `commit_renders_transcript_line_into_buffer`; use `backend_mut()` for the
  cursor call. `Position` has public `x`/`y` fields.
- Existing tests that pin today's layout and must keep passing:
  `input_box_row_is_stable_across_draw_modes`,
  `wrapped_multiline_input_renders_across_rows`,
  `multiline_buffer_renders_with_cursor`, `tall_body_scrolls_cursor_into_view`
  (these assert text presence, not cursor coordinates — they are compatible
  with the new pre-wrapped rendering as long as wrapped text still appears
  across rows).

## Spec

### Task 1 — Pre-wrap the input text with `visual_lines` in `draw`

In `src/cli/render_ratatui.rs`, rewrite `draw` so the rendered text is built
from the same wrapper as the cursor — build both **inside** the draw closure
where `area.width` is known:

```rust
pub fn draw(&mut self, input: &InputLine, status: &StatusBarState<'_>) -> Result<(), B::Error> {
    let session_id = status.session_id.to_string();
    let model = status.model.to_string();
    let start_time = self.start_time;

    let _completed = self.terminal.draw(|frame| {
        let area = frame.area();
        let content_width = area.width.saturating_sub(2) as usize;
        // One wrapper for glyphs and cursor: visual_lines is the authority.
        let visual: Vec<String> = input
            .visual_lines(content_width)
            .into_iter()
            .map(|l| l.into_iter().collect())
            .collect();
        let input_text: ratatui::text::Text<'static> = visual
            .into_iter()
            .map(|l| Line::from(Span::raw(l)))
            .collect();
        let (vis_row, vis_col) = input.cursor_visual_pos(content_width);
        let cursor_pos = Some((vis_col as u16, vis_row as u16));

        render_live_region(
            frame,
            area,
            &input_text,
            &session_id,
            &model,
            start_time,
            cursor_pos,
        );
    })?;
    Ok(())
}
```

(The `input.visual_lines(content_width)` call is pinned — it is mutation M2's
target.)

### Task 2 — Stop double-wrapping in `render_live_region`

The text arriving from Task 1 is already wrapped to the inner width, so remove
`.wrap(Wrap { trim: false })` from the **input** paragraph (`:501-504`):

```rust
let input_para = Paragraph::new(input_text.clone())
    .block(input_block)
    .scroll((scroll_offset, 0));
```

Keep `.scroll` — the vertical scroll logic is unchanged and now operates on
exact visual rows. Do **not** touch the `.wrap` in `render_prompt_region`
(`:589-591`); the `Wrap` import stays.

### Task 3 — Fix the border clamp

In the same function's cursor block, change both clamps from
`saturating_sub(2)` to `saturating_sub(3)`:

```rust
let x = content_area.x + 1 + col.min(content_area.width.saturating_sub(3));
let y =
    content_area.y + 1 + (visible_row as u16).min(content_area.height.saturating_sub(3));
```

(Inner content occupies offsets `1 ..= width-2` inside the box; the max
*content* column is `width-2`, reached as `x + 1 + (width-3)`. This is
mutation M1's target.)

### Task 4 — One width for key handling

1. In `render_ratatui.rs`, add a small accessor near `reanchor`:

   ```rust
   /// The input box's inner content width — the same value `draw` wraps
   /// with. Key handling must use this, not the tmux/ioctl-derived width.
   pub fn input_content_width(&self) -> usize {
       self.terminal
           .size()
           .map(|s| s.width.saturating_sub(2) as usize)
           .unwrap_or(78)
   }
   ```

2. In `src/cli/commands/chat.rs`, replace **both**
   `let content_width = chat_width.saturating_sub(2);` sites (the `Key::Up`
   and `Key::Down` arms) with:

   ```rust
   let content_width = renderer.input_content_width();
   ```

   `chat_width` itself stays — the banner centering and the IPC
   `chat_width` field still use it.

### Task 5 — Tests

Write the tests named in § Test plan in `render_ratatui.rs`'s existing
`mod tests`, using the `make_test_renderer` helper (TestBackend 60×10,
`Viewport::Inline(6)`, inner content width = 58). Read the cursor after a
`draw` with `renderer.terminal.backend_mut().get_cursor_position().unwrap()`.
Locate the input-box top row with the existing `corner_row` helper rather
than hardcoding a row index.

### Task 6 — Mutation M1 apply + restore (clamp)

Apply a `patch` on `src/cli/render_ratatui.rs` changing
`col.min(content_area.width.saturating_sub(3))` to
`col.min(content_area.width.saturating_sub(2))`, then:

```sh
echo "== M1 APPLIED ==" >> /tmp/e2e-m13-04.txt
cargo test --lib cursor_clamp_never_reaches_border 2>&1 | tail -5 >> /tmp/e2e-m13-04.txt
```

The test must show **FAILED**. If it stays green, report a blocker — do not
adjust a test to make it fail. Restore with the inverse `patch`, then:

```sh
echo "== M1 RESTORED ==" >> /tmp/e2e-m13-04.txt
grep -c 'col.min(content_area.width.saturating_sub(3))' src/cli/render_ratatui.rs >> /tmp/e2e-m13-04.txt
cargo test --lib cursor_clamp_never_reaches_border 2>&1 | tail -5 >> /tmp/e2e-m13-04.txt
```

The grep count must be `1` and the test green.

### Task 7 — Mutation M2 apply + restore (wrapper agreement)

Apply a `patch` on `src/cli/render_ratatui.rs` changing
`input.visual_lines(content_width)` to
`input.visual_lines(content_width + 1)`, then:

```sh
echo "== M2 APPLIED ==" >> /tmp/e2e-m13-04.txt
cargo test --lib cursor_matches_glyph 2>&1 | tail -5 >> /tmp/e2e-m13-04.txt
```

`cursor_matches_glyph_on_word_wrapped_input` must show **FAILED** (glyphs wrap
one column later than the cursor math). Restore with the inverse `patch`,
then:

```sh
echo "== M2 RESTORED ==" >> /tmp/e2e-m13-04.txt
grep -c 'visual_lines(content_width + 1)' src/cli/render_ratatui.rs >> /tmp/e2e-m13-04.txt
cargo test --lib cursor_matches_glyph 2>&1 | tail -5 >> /tmp/e2e-m13-04.txt
```

The grep count must be `0` and the tests green.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m13-04.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Progress markers — each **fails against the current tree** (verified at
drafting):

- [ ] `grep -c 'wrap(Wrap { trim: false })' src/cli/render_ratatui.rs` prints
      `1` — only the prompt region's. (Currently: 2.)
- [ ] `grep -c 'chat_width.saturating_sub(2)' src/cli/commands/chat.rs`
      prints `0`. (Currently: 2.)
- [ ] `grep -c 'saturating_sub(3)' src/cli/render_ratatui.rs` prints `2` —
      the two clamp fixes. (Currently: 0.)
- [ ] `grep -c 'input_content_width' src/cli/commands/chat.rs` prints `2` and
      `grep -c 'pub fn input_content_width' src/cli/render_ratatui.rs` prints
      `1`. (Currently: 0 and 0.)
- [ ] Tests `cursor_sits_on_next_free_cell_of_short_input`,
      `cursor_matches_glyph_on_word_wrapped_input`,
      `cursor_clamp_never_reaches_border`,
      `input_content_width_matches_draw_width` pass. (Currently: none exist.)

No-regression guards — these **already pass** and must still pass (they are
not evidence of new work):

- [ ] `input_box_row_is_stable_across_draw_modes`,
      `wrapped_multiline_input_renders_across_rows`,
      `multiline_buffer_renders_with_cursor`,
      `tall_body_scrolls_cursor_into_view` still pass.
- [ ] The `editor.rs` `visual_lines_*` / `cursor_visual_pos_*` suites still
      pass untouched (this phase must not edit `editor.rs`).
- [ ] Four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

All in `src/cli/render_ratatui.rs` `mod tests` (60×10 TestBackend, inner
width 58). After each `draw`, read
`renderer.terminal.backend_mut().get_cursor_position().unwrap()` and locate
the box with `corner_row`.

- `cursor_sits_on_next_free_cell_of_short_input` — type `"hello"` (cursor at
  end, row 0, col 5); assert the cursor is at exactly
  `(x: 1 + 5, y: box_top + 1)` **and** the buffer cell at `(1 + 4, box_top + 1)`
  holds `o` — the cursor is one cell right of the last glyph, same row.
- `cursor_matches_glyph_on_word_wrapped_input` — input = 50 `a`s, a space,
  then 10 `b`s (60 visible chars: word-wrap carries the whole `b` word to
  visual row 1; a char-wrap would split it). Assert the cursor lands at
  `(x: 1 + 10, y: box_top + 2)` and the buffer cell at `(1, box_top + 2)`
  holds `b` — cursor and glyphs agree on *where the wrap happened*.
  (Mutation M2 target.)
- `cursor_clamp_never_reaches_border` — input long enough that the unclamped
  column would exceed the box (e.g. one unbroken 58-char word, cursor at
  end): assert `cursor.x <= 1 + 56` (max content column) and that the cell at
  `(59, cursor.y)` — the border column — is `│` or untouched, i.e. the cursor
  x is strictly less than 59. (Mutation M1 target.)
- `input_content_width_matches_draw_width` —
  `renderer.input_content_width()` == 58 on the 60-wide TestBackend.

## End-to-end verification

```sh
: > /tmp/e2e-m13-04.txt
echo "== GATES ==" >> /tmp/e2e-m13-04.txt
cargo fmt --all 2>&1 | tail -2 >> /tmp/e2e-m13-04.txt; echo "fmt exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-04.txt
cargo build 2>&1 | tail -2 >> /tmp/e2e-m13-04.txt; echo "build exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-04.txt
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2 >> /tmp/e2e-m13-04.txt; echo "clippy exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-04.txt
cargo test 2>&1 | grep -E '^test result' >> /tmp/e2e-m13-04.txt; echo "test exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-04.txt
echo "== SURFACES ==" >> /tmp/e2e-m13-04.txt
echo "wrap calls: $(grep -c 'wrap(Wrap { trim: false })' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-04.txt
echo "stale widths: $(grep -c 'chat_width.saturating_sub(2)' src/cli/commands/chat.rs)" >> /tmp/e2e-m13-04.txt
echo "clamps: $(grep -c 'saturating_sub(3)' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-04.txt
wc -l /tmp/e2e-m13-04.txt >> /tmp/e2e-m13-04.txt
```

(Note the exit markers use `${PIPESTATUS[0]}` — the *command's* exit code, not
the `tail`/`grep` pipe's. The mutation runs of Tasks 6-7 append into the same
file in task order.)

A live feel-check (cursor on the edited character while typing a wrapped
multi-line input in real tmux) happens at milestone close.

## Authorizations

None.

## Out of scope

- `render_prompt_region` and `draw_prompt` — the approval-prompt path keeps
  `Paragraph::wrap` and sets no cursor; untouched.
- `src/cli/input/editor.rs` — `visual_lines` / `cursor_visual_pos` are the
  authority precisely because they are already tested; do not edit them.
- Resize/re-anchor behavior (phase 05), `render.rs` dead-code deletion
  (phase 05).
- `chat_width`'s other uses (banner centering, IPC field) stay.
- Anything under `src/daemon/`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
