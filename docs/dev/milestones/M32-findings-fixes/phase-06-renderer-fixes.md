# Phase 06 — Panel borders fill by display width

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Findings:** M31 `FINDINGS.md` #57 and #60 (#58 is rejected below; #55
and #56 move to phase 12)

## Goal

`commit_panel_labeled` (`src/cli/render_ratatui_draw.rs`) draws a
panel's top and bottom borders by hand. The dash fill is computed from
`title.chars().count()` and `padded.chars().count()`, but the terminal
lays out cells. A double-width title or bottom label (CJK, many emoji)
therefore makes the border wider than the terminal (#57). On a
60-column terminal, the title `漢字 title` gives a 62-cell top border.

After this phase, both widths come from ratatui's `Span::width()`, which
measures display cells, so both border rows are exactly as wide as the
terminal. The module doc's first line also gets its closing backtick
(#60).

**#58 is rejected.** `commit`, `commit_styled` and
`commit_panel_labeled` raise `inserted_rows` and record history before
they know whether `insert_before` succeeded. When it fails, the error
propagates and the caller tears the renderer down. The counters are
never read again, so nothing observable goes wrong, and ratatui's
`TestBackend` cannot fail, so no test could show a difference.

**#55 and #56 move to phase 12** (client-prompts): the three panels'
copied status-bar tail, and the picker panel's border. They belong with
the other `StatusBarState` consolidation there.

## Architecture references

- M32 README, decisions 1 and 6.

## Spec

1. **Add the test below, and run it red.** Append it to the end of
   `src/cli/render_ratatui_tests.rs`, then run
   `cargo test --lib commit_panel_borders_fill_by_display_width`. It must
   fail with `left: 62` / `right: 60` on the top border. Paste that
   output into the Update Log.
2. **The fix.** In `commit_panel_labeled`, change
   `let title_len = title.chars().count();` to
   `let title_len = Span::raw(title).width();`, and change
   `let label_vis = padded.chars().count();` to
   `let label_vis = Span::raw(padded.as_str()).width();`. Run the test
   again; it must pass.
3. **The doc.** On line 1 of `render_ratatui_draw.rs`, change
   `` `RatatuiRenderer's `` to `` `RatatuiRenderer`'s ``.
4. **Findings** go in their own `### Findings — <date>` entry, from
   `commit_panel_labeled` only, within five tool calls, or the single
   line `none`.
5. **Commit** as `fix:`.

**Do not look further.** Nothing else in the renderer changes. The
architect dry-ran this exact change on `HEAD`: red at step 1, green at
step 2, clippy and fmt clean, and the full suite at
`passed=2600 failed=0`.

```rust
/// A panel's border rows are as wide as the terminal even when the title
/// or the bottom label holds double-width characters.
#[test]
fn commit_panel_borders_fill_by_display_width() {
    let mut renderer = make_test_renderer();
    renderer
        .commit_panel_labeled("漢字 title", &["body".to_string()], false, Some("ラベル"))
        .unwrap();
    let rows: Vec<&Line<'static>> = renderer.history.iter().collect();
    let top = rows[0];
    let bottom = rows[rows.len() - 2];
    assert_eq!(top.width(), 60, "top border: {top:?}");
    assert_eq!(bottom.width(), 60, "bottom border: {bottom:?}");
}
```

## Acceptance criteria

- [ ] `grep -c 'chars().count()' src/cli/render_ratatui_draw.rs` prints
      `0` (today: `2`).
- [ ] `head -1 src/cli/render_ratatui_draw.rs` contains
      `` `RatatuiRenderer`'s `` (today it does not).
- [ ] `commit_panel_borders_fill_by_display_width` passes, and the
      Update Log shows it failing at step 1.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2600 failed=0` (2599 + 1).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
F=src/cli/render_ratatui_draw.rs
echo "char_counts=$(grep -c 'chars().count()' $F) doc=$(head -1 $F | grep -c 'RatatuiRenderer`.s')"
cargo test --lib commit_panel 2>&1 | grep -E '^test result'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `char_counts=0 doc=1`, `test result: ok. 6 passed` for the
`commit_panel` tests, hygiene `ok. 16 passed`, bug tracker `ok. 6
passed`, clippy clean, `passed=2600 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flake:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If the
block shows `passed=2599 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/cli/render_ratatui_draw.rs` (the two lines and the doc line)
  and append to `src/cli/render_ratatui_tests.rs`. No other source file.

## Out of scope

- Body lines, which `wrap_line_hard` wraps by chars.
- #58 (rejected above); #55 and #56 (phase 12).

## Update Log

<!-- entries appended below this line -->
