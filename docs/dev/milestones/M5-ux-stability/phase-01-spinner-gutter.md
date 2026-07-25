# Phase 01: Spinner Row

**Milestone:** M5 — UX & Stability
**Status:** in-progress
**Depends on:** none
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Move the streaming spinner out of the chat input box and onto a dedicated
one-row line immediately **above** the box's top border. The row is reserved in
every live-region draw mode — blank when idle — so the input box never shifts
vertically when streaming starts or stops. The full terminal width is available
on that row, so the animated frame, the verb, and the dot animation all render
together outside the box.

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 2.1 — the defect and why all three
  live-region renderers must agree on the reserved row.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Everything in this phase lives in `src/cli/render_ratatui.rs` (1209 lines). No
other file changes.

The live region is an inline ratatui viewport six rows tall:

```rust
// src/cli/render_ratatui.rs:119
const VIEWPORT_ROWS: u16 = 6;
```

There are **three** functions that draw that region. All three split it
vertically and give the input box everything above the status bar:

```rust
// src/cli/render_ratatui.rs:409 — normal input mode
fn render_live_region(
    frame: &mut ratatui::Frame,
    area: Rect,
    input_text: &ratatui::text::Text<'_>,
    session_id: &str,
    model: &str,
    start_time: std::time::Instant,
    cursor_pos: Option<(u16, u16)>, // (col, row) within content area (before scroll)
) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);

    // ── Input box ──────────────────────────────────────────────
    let content_area = chunks[0];
```

```rust
// src/cli/render_ratatui.rs:545 — streaming mode; the defect
fn render_spinner_region(
    frame: &mut ratatui::Frame,
    area: Rect,
    spinner_line: Line<'static>,
    session_id: &str,
    model: &str,
    start_time: std::time::Instant,
) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);

    // ── Spinner line inside the input box ──────────────────────
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::Gray));
    let input_para = Paragraph::new(spinner_line).block(input_block);
    frame.render_widget(input_para, chunks[0]);
```

That last block is the bug: the spinner line is rendered *as the content of the
bordered input block*, so it squats inside the box and replaces the user's
input text instead of appearing above it.

`render_prompt_region` (line 477) is the third mode. It already reserves rows
at the top for a prompt string, and it carries a short-region fallback:

```rust
// src/cli/render_ratatui.rs:486
    // Reserve 1 row for status bar, 2 for input box, rest for prompt.
    let total = area.height;
    if total < 4 {
        // Too small — fall back to normal input region.
        …
        render_live_region(frame, area, &it, session_id, model, start_time, None);
        return;
    }
    let prompt_rows = total - 3; // 1 status + 2 input box
```

The spinner `Line` is fully assembled by the caller, `draw_spinner` (line 235),
including the parenthesised frame, the verb, and the dots:

```rust
// src/cli/render_ratatui.rs:257
let spinner_line = Line::from(vec![
    Span::raw("  "),
    Span::styled(open, blood_red),
    Span::styled(center, bright_yellow),
    Span::styled(close, blood_red),
    Span::styled(format!(" {verb}"), blood_red),
    Span::styled(".".repeat(dot_count), bright_yellow),
]);
```

This line stays exactly as it is — frame, verb, and dots travel together. Only
*where* it is rendered changes.

The frames and verbs it animates come from `src/cli/commands/stream.rs:123`:
`SPINNER = ["(─)", "(○)", "(◎)", "(◉)", "(◎)", "(○)"]` and ten verbs
(`"scrying"`, `"beholding"`, `"discerning"`, …), longest `"discerning"` at 10
characters. With two leading spaces, a 3-cell frame, a space, the verb, and up
to three dots, the line needs about 20 columns — hence a full-width row rather
than a left gutter.

Callers (do **not** change them): `draw_spinner` is called from
`src/cli/commands/stream.rs:217`, `:234`, `:273`; `draw_prompt` from
`stream.rs:779` and eight other sites. All three public methods (`draw`,
`draw_spinner`, `draw_prompt`) keep their current signatures.

## Spec

### 1. Add the reserved-row constant and a shared split helper

In `src/cli/render_ratatui.rs`, next to `const VIEWPORT_ROWS: u16 = 6;` (line
119), add:

```rust
/// Rows reserved above the input box for the streaming spinner line. The row
/// is always reserved — blank when idle — so the input box never moves
/// vertically when streaming starts or stops.
const SPINNER_ROWS: u16 = 1;

/// Minimum live-region height at which the spinner row is reserved. Below
/// this the row collapses so a very short region still gets a usable box.
const MIN_HEIGHT_FOR_SPINNER_ROW: u16 = 5;

/// Split a live-region area into (spinner_row, body). The spinner row is
/// reserved in every draw mode; `body` is what the existing vertical layouts
/// then split into input box and status bar. On a short region the spinner
/// row is zero-height.
fn split_spinner_row(area: Rect) -> (Rect, Rect) {
    if area.height < MIN_HEIGHT_FOR_SPINNER_ROW {
        let empty = Rect { height: 0, ..area };
        return (empty, area);
    }
    let chunks =
        Layout::vertical([Constraint::Length(SPINNER_ROWS), Constraint::Min(1)]).split(area);
    (chunks[0], chunks[1])
}
```

No new imports — `Constraint`, `Layout`, and `Rect` are already imported at
line 5.

### 2. Reserve the row in `render_live_region`

Call `split_spinner_row(area)` **first**, then run the existing vertical split
on the returned `body` rect instead of on `area`. Render nothing into the
spinner rect — it stays blank in this mode. Everything else (input box, cursor,
status bar) is unchanged apart from deriving from `body`.

The cursor math at lines 448–453 already derives from `content_area.x` / `.y`,
so it stays correct **as long as** `content_area` comes from the post-split
`body`. Do not reintroduce `area` there.

Note the box's content height shrinks by one row. The existing `scroll_offset`
logic (lines 422–435) already derives `content_height` from
`content_area.height`, so multi-line input keeps scrolling correctly with no
change — provided `content_area` is the new, shorter rect.

### 3. Render the spinner into the reserved row in `render_spinner_region`

Call `split_spinner_row(area)`. Then:

- Render `spinner_line` as a plain `Paragraph` into the **spinner rect** — no
  `Block`, no borders. Full width is available; leave the line's existing
  leading `Span::raw("  ")` pad in place so it sits two columns in.
- Render the bordered input box into `body`'s first chunk with **empty**
  content (`Paragraph::new("")` with the same `Block` construction used today).
  The box keeps its border and position; it simply shows nothing while
  streaming. Do **not** change `draw_spinner`'s signature to accept an
  `InputLine`.
- Render the status bar into `body`'s second chunk exactly as today.

When the spinner rect is zero-height (short-region fallback from task 1),
ratatui clips the paragraph to nothing automatically — no special-casing, and
no panic.

### 4. Reserve the row in `render_prompt_region`

Call `split_spinner_row(area)` first, then run the existing three-way vertical
split (prompt rows / input box / status bar) on `body`. Leave the spinner rect
blank — a spinner and a modal prompt are never on screen at the same time; the
row is reserved only so the box does not jump when the mode changes.

Update the short-region fallback at line 486 to measure `body`, not `area`:
compute `let total = body.height;` and keep the existing `if total < 4` guard
and `let prompt_rows = total - 3;` arithmetic. The guard's threshold does not
change — it is now applied to a rect that is already one row shorter, which is
the correct behavior.

### 5. Confirm `VIEWPORT_ROWS` stays at 6

Do **not** change `VIEWPORT_ROWS`. The spinner row is taken out of the existing
six-row viewport, not added to it, so the live region occupies the same amount
of the user's terminal as it does today. The input box goes from three content
rows to two; longer input scrolls, which the existing `scroll_offset` logic
already handles.

This is a deliberate trade — a permanently taller viewport would steal a
terminal row even when idle. It is a one-constant change if it proves wrong in
use, but it is **out of scope** for this phase.

### Approved layout

Streaming — spinner, verb, and dots together on the reserved row, above the
box:

```
  (◉) scrying...
┌────────────────────────────┐
│                            │
└────────────────────────────┘
 session:a1b2… · opus · up 3m
```

Idle — the row is still reserved, so the box has **not** moved:

```

┌────────────────────────────┐
│ type here                  │
└────────────────────────────┘
 session:a1b2… · opus · up 3m
```

## Acceptance criteria

- [ ] `cargo fmt --all` clean; `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits zero.
- [ ] `cargo test` green, including the pre-existing
      `live_region_shows_input_text_and_status_bar` and
      `commit_renders_transcript_line_into_buffer`.
- [ ] Test `spinner_renders_above_input_box_not_inside_it` passes.
- [ ] Test `input_box_row_is_stable_across_draw_modes` passes.
- [ ] Test `spinner_row_is_blank_when_idle` passes.
- [ ] Test `short_region_collapses_spinner_row` passes.
- [ ] No changes to any file other than `src/cli/render_ratatui.rs`.

## Test plan

Add to the existing `mod tests` at `src/cli/render_ratatui.rs:616`. Reuse the
`make_test_renderer()` helper (line 621) and copy the nine-field
`StatusBarState` literal from `live_region_shows_input_text_and_status_bar`
(line 636) rather than inventing field values.

Read cells positionally with ratatui 0.30's `Buffer` index impl:
`renderer.terminal.backend().buffer()[(x, y)].symbol()` — indexing by
`(u16, u16)` is supported in this version.

**Do not hardcode row numbers.** The renderer draws into an inline viewport
whose origin is not guaranteed to be `y == 0`. Locate the box by scanning for
the row containing `'┌'` and assert *relative* to it. A helper like
`fn corner_row(buf: &Buffer) -> u16` returning the y of the first `'┌'`, plus
one that collects a whole row into a `String`, keeps all four tests short.

- `spinner_renders_above_input_box_not_inside_it` in
  `src/cli/render_ratatui.rs` — after
  `draw_spinner("(◉)", "scrying", 3, &status)`:
  - asserts the row at `corner_row - 1` contains `"scrying"` and `"..."` —
    verb and dots travel with the frame;
  - asserts that same row contains the frame's centre glyph `'◉'`;
  - asserts the rows **at and below** `corner_row` do **not** contain
    `"scrying"` — negative pin proving the spinner left the box interior. This
    is the assertion that fails today.

- `input_box_row_is_stable_across_draw_modes` in
  `src/cli/render_ratatui.rs` — the exit-criterion test. On one renderer, call
  `draw(&input, &status)` and record `corner_row`; then
  `draw_spinner("(◉)", "scrying", 1, &status)` and record it again; then
  `draw_prompt("password:", &input, &status)` and record a third time. Assert
  all three are equal. A failure means the box jumps vertically when streaming
  starts, which is what the reserved row exists to prevent.

- `spinner_row_is_blank_when_idle` in `src/cli/render_ratatui.rs` — after
  `draw(&input, &status)` with input `"Hello"`, assert the row at
  `corner_row - 1` is entirely whitespace. Negative pin: the reserved row must
  not leak residue from a previous spinner draw or from the box border.

- `short_region_collapses_spinner_row` in `src/cli/render_ratatui.rs` — build a
  renderer whose viewport is shorter than `MIN_HEIGHT_FOR_SPINNER_ROW` (a
  `TestBackend::new(60, 10)` with `Viewport::Inline(4)`), then call `draw` and
  `draw_spinner`. Asserts neither panics and a `'┌'` is still present. Pins the
  fallback: a short region keeps a usable box rather than losing a row it
  cannot spare.

## End-to-end verification

Unit tests here run against `TestBackend`, a hermetic fake — they can pass
while the real terminal output is wrong. Before reporting complete, run the
real binary in tmux and confirm by eye:

```sh
cargo build --release
tmux new-session -d -s de-phase01 './target/release/daemoneye daemon --console'
# in a second pane of that session:
./target/release/daemoneye chat
```

Send one query, then watch the transition. Confirm and quote in the Update Log:

1. While idle, there is one blank row directly above the input box's top
   border.
2. While the response streams, that row shows the animated frame **with** its
   verb and dots (e.g. `  (◉) scrying...`), entirely outside the box border.
3. The box's top border is on the **same screen row** in both states — it does
   not jump when streaming starts or stops.

`tmux capture-pane -p -t <pane>` gives a text snapshot; paste one per state
into the Update Log as evidence.

## Authorizations

None. No new dependencies; `docs/architecture.md` is not touched.

## Out of scope

- **Do not** change what the spinner says or how it animates — the `SPINNER`
  frame table, the `VERBS` table, `TICKS_PER_VERB`, and the dot-count logic in
  `src/cli/commands/stream.rs` all stay exactly as they are. This phase moves
  the line; it does not restyle it.
- **Do not** split the verb or dots out of `spinner_line`. Frame, verb, and
  dots stay in one `Line`, rendered together.
- **Do not** change `VIEWPORT_ROWS` (see task 5).
- **Do not** change any `draw`/`draw_spinner`/`draw_prompt` call site in
  `src/cli/commands/stream.rs`. The public signatures are unchanged, so no
  caller needs an edit.
- **Do not** touch `commit`, `commit_styled`, or `commit_panel`. Committing the
  user's input to scrollback is phase 02's job; if you find yourself editing
  `chat.rs`, you have left this phase.
- **Do not** add a left-hand gutter or otherwise change the input box's
  horizontal position. The box's width and columns are unchanged by this phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-25 07:22 (started)

**Executor:** Claude (Sonnet 4.5)
**Status:** in-progress

Implemented the spinner-row reservation across all three live-region renderers. Added `SPINNER_ROWS`, `MIN_HEIGHT_FOR_SPINNER_ROW`, and `split_spinner_row()` helper. Updated `render_live_region`, `render_spinner_region`, and `render_prompt_region` to reserve the spinner row. Added four new tests: `spinner_renders_above_input_box_not_inside_it`, `input_box_row_is_stable_across_draw_modes`, `spinner_row_is_blank_when_idle`, `short_region_collasses_spinner_row`.
