# Phase 01 — input-box-paste

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-01
**Depends on:** none
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Design item 1 (`docs/design/2.7.0-UX-improvements.md`): the chat input box
draws `│` on both sides of every row, so selecting several lines of it in the
terminal copies `│ text │` lines. After this phase the box keeps its top and
bottom rules and draws a **blank column** where each side border was. The
text, the wrap width and the cursor stay exactly where they are today.

## Architecture references

- `docs/architecture.md` § 1 System layers — the chat client's renderer.
- `src/cli/render_ratatui.rs` — `render_live_region`, `render_prompt_region`
  and `render_spinner_region`: the three places the input box is built, each
  with its own `Block::default().borders(Borders::ALL)…`.
- `src/cli/render_ratatui_draw.rs` — `input_content_width` (wrap width =
  terminal width − 2), which this phase must not change.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the three render functions above and the tests in
   `src/cli/render_ratatui_tests.rs` and
   `src/cli/render_ratatui_layout_tests.rs` that look for `┌` or `│`.
3. Read this entire phase doc before touching any code.

## Constraints

- **One helper builds the input box's block**, used by all three render
  functions, so the three cannot drift again.
- **Keep the inner area identical.** With `Borders::TOP | Borders::BOTTOM`
  alone, ratatui's `Block::inner` no longer insets the left and right
  columns, so text would move one column left and the wrap width would grow
  by two. ratatui 0.30's `Block` has `.padding(Padding::horizontal(1))`
  (`ratatui::widgets::Padding`), and `inner()` subtracts padding after
  borders — that keeps today's inner rectangle. The cursor arithmetic in
  `render_live_region` (`content_area.x + 1 + …`) and `input_content_width`
  must need no change; if either has to change, the inner area moved.
- **The rules have no corners.** ratatui draws a top-only or bottom-only
  border as a plain `─` run across the full width. That is the intended look.
- **Only the input box changes.** `alert_panel` (approval and credential
  panels, rounded red borders), the picker panel in `render_ratatui_draw.rs`,
  and the panels `commit_panel` / `commit_panel_labeled` write into
  scrollback keep their borders exactly as they are.
- **Tests that locate the box by `┌`** (the `find_box_top` helper in
  `render_ratatui_layout_tests.rs`, and its siblings) must find it another
  way: the box's top rule is the first row made entirely of `─`. Committed
  panels in history never produce such a row, because their top and bottom
  rows carry corners. Assertions that require `┌` or a side `│` on the input
  box assert the old behaviour and are inverted, not deleted.
- **Use `patch` with whole blocks** on these files, never `patch_lines`.

## Spec

1. **Red tests first** — in `src/cli/render_ratatui_tests.rs` (or the layout
   tests file, your choice), write the three tests below against the current
   code, run them, and keep the failing output for step 5:
   - `paste_safe_box_has_blank_side_columns` — draw the idle live region with a
     three-line input on a `TestBackend`. On every row between the box's top
     and bottom rules, the first and last columns are `" "`, and no cell in
     the live region is `│`.
   - `paste_safe_box_blank_under_spinner_and_prompt` — the same
     check for the spinner region (with typeahead text) and the prompt region
     (with a prompt string and input text).
   - `paste_safe_box_keeps_top_and_bottom_rules` — the rows directly above and
     below the box's content are `─` in every column.
2. **The helper** — add one function in `render_ratatui.rs` that returns the
   input box's `Block` (top and bottom borders, plain type, the existing gray
   border style, horizontal padding 1), and use it in all three render
   functions. The three new tests pass.
3. **Existing tests** — update `find_box_top` and every assertion that needs
   `┌` or a side `│` on the input box, per the constraint above. Every test
   that pins the cursor column, `input_content_width` (58 on a 60-wide
   backend) or the border column must still pass **unchanged in what it
   asserts about positions**; if one fails, the inner area moved — fix the
   block, not the test.
4. **Full suite** — `cargo test` passes with `failed=0`.
5. **Red runs** — one `### Update — <date> (red runs)` entry pasting step 1's
   `test result:` lines and assertion messages.
6. **Findings** — one `### Findings — <date>` entry: anything you noticed
   outside this phase's scope, two or three lines, or `none`.
7. **End-to-end** — run the block in § End-to-end verification once before
   committing and paste its output, fenced, in an
   `### Update — <date> (end-to-end verification)` entry. Required; the
   reviewer checks for it.
8. **Commit** as `feat: the chat input box has blank side columns, so a multi-line copy has no │ (M36 phase-01)`.

## Acceptance criteria

- [ ] `paste_safe_box_has_blank_side_columns`,
      `paste_safe_box_blank_under_spinner_and_prompt` and
      `paste_safe_box_keeps_top_and_bottom_rules` exist and pass, and the red-runs
      entry shows them failing on the pre-phase code.
- [ ] `input_content_width_matches_draw_width`,
      `cursor_sits_on_next_free_cell_of_short_input`,
      `cursor_matches_glyph_on_word_wrapped_input` and
      `cursor_clamp_never_reaches_border` pass with their position assertions
      unchanged (preservation; the last one's border-cell check already
      accepts `" "`).
- [ ] The approval panel tests (`approval_panel_*`) pass unchanged
      (preservation: `alert_panel` keeps its rounded border).
- [ ] The E2E block's first line is `paste_safe passed=3 failed=0`
      (today `paste_safe passed=0 failed=0`).
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0`, and `failed=0`.

## Test plan

The three new tests carry the behaviour; the preserved position tests prove
the inner area did not move. No live run is needed in this phase — the
milestone's live check pastes three lines into the box on pinky.

## End-to-end verification

```sh
bash <<'E2E'
cargo test paste_safe_box_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "paste_safe passed=" p, "failed=" f}'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/render_ratatui.rs`, `src/cli/render_ratatui_tests.rs` and
  `src/cli/render_ratatui_layout_tests.rs`. `src/cli/render_ratatui_draw.rs`
  only if a test helper there locates the box by `┌`.

## Out of scope

- The status bar under the box (phase 02).
- Borders of committed scrollback panels (the user-echo panel and tool
  output keep their `│`), the approval and credential panels, and the picker.
- Any change to the input editor, wrapping or cursor logic.

## Update Log

<!-- entries appended below this line -->
