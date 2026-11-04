# Bug 1 on phase-04b: the first shell row can never be attached

**Severity:** major
**Status:** fixed (d8da67cc, approved 2026-11-04)
**Filed:** 2026-11-03

## What's wrong

`TopModel::attach_candidate` (`src/cli/commands/top_tui.rs`, the
`.filter(|_| self.shells_selected > 0)` line) returns `None` whenever the
selection is on row 0. Phase 04a's selection model treats row 0 as
selected — `shells_selected_line` (`top_tui_draw.rs`) draws it reversed
the moment Shells is focused — so the user sees the first shell selected,
presses `Enter` or `a`, and nothing happens. With exactly one shell, attach
is impossible; with several, the top row is unreachable.

`top_attach_enter_or_a_opens_only_on_a_selected_shell` pins the defect: it
asserts `attach_candidate() == None` on a fresh model with two shells
("no selection yet, nothing to open"), and the Findings entry restates it
as intended.

## What should happen

Phase doc § Behaviour: "With Shells focused and a shell selected, `Enter`
or `a` opens the attach view on it." The drawn selection is the
selection: with Shells focused and at least one row, the reversed row is
the one `Enter`/`a` opens, row 0 included. Still nothing with Runs focused,
no rows, or the filter being edited.

## Root cause

The `shells_selected > 0` guard treats index 0 as "unmoved" rather than
as the first row; 04a never had an "unselected" state.

## Definition of done

- [x] `top_attach_enter_on_the_first_shell_opens_it` (new) passes: one
      shell, Shells focused, no movement — `Enter` and `a` each request the
      open on `s1` (fails today).
- [x] The trigger test no longer asserts `None` for an unmoved selection on
      a populated panel.
