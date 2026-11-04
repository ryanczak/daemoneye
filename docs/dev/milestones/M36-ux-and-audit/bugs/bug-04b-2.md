# Bug 2 on phase-04b: the footer has no attach hint and never shows an open error

**Severity:** major
**Status:** open
**Filed:** 2026-11-03

## What's wrong

The footer format in `src/cli/commands/top_tui_draw.rs` (the
`"q quit · ↑↓ scroll · tab focus · …"` line) is unchanged by
`508906f3`. `grep -rn 'enter attach' src/cli/commands/top*` finds nothing,
and `TopModel::attach_note` is written (`open_requested_attach`) and
cleared (`handle_key`) but read by no drawing code — a failed open (daemon
down, shell gone) leaves the panels with no message at all.

The completion summary states "the footer names `enter attach` while Shells
has rows and `attach failed: <msg>` when an open fails"; neither string is
in the tree. `top_attach_open_error_leaves_the_panels_and_names_it` sets
`attach_note` by hand and asserts on the field, never on the drawn screen,
so it cannot catch this.

## What should happen

Phase doc § Behaviour: "The footer names `enter attach` while Shells is
focused and has rows" and, on a failed open, "the panels stay and the
footer shows the error until the next key."

## Root cause

Spec step 2's footer work was not done; the test pins the model field, not
the rendering.

## Definition of done

- [ ] `top_attach_footer_names_enter_attach_and_the_open_error` (new)
      passes on a `TestBackend`: `enter attach` in the footer with Shells
      focused and rows, absent with Runs focused; with `attach_note` set,
      the drawn footer shows the message, and after the next key it is gone
      (fails today).
