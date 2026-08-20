# Bug 2 on phase-03: the wrap guards pass when the wrap wiring is reverted

**Severity:** minor (shipped behaviour is correct; the guards do not hold it)
**Status:** resolved (round 3, `3fee471`)
**Filed:** 2026-08-20
**Found by:** reviewer mutation during the round-2 review.

## What's wrong

Round 2 fixed the real defect — prose now word-wraps, `Block::Output` keeps the
hard wrap, and `Modifier::UNDERLINED` is gone. **The code is right.** But two of
its four new guards do not detect the regression they exist to prevent.

Reviewer mutations, each undoing the user-visible fix, run against the round-2
tree:

| Mutation | Expected | Observed |
|---|---|---|
| `viewer.rs:273` `push_wrapped_hard` → `push_wrapped` (re-flow machine output) | a test fails | **41/41 pass** |
| `viewer.rs:242,245` `push_wrapped` → `push_wrapped_hard` (prose back to mid-word cuts) | a test fails | **41/41 pass** |
| `style_for_focused` returns the unfocused style | a test fails | fails `style_for_focused_is_distinct_without_underline` ✓ |

Two causes:

1. **`wrap_words_does_not_split_words` tests the helper, not the wiring.** It
   calls `wrap_words(text, 12)` directly and never goes through
   `layout_blocks`, so which wrapper `layout_block` actually calls for prose is
   unasserted.
2. **`output_rows_keep_hard_wrap` uses a fixture both strategies handle
   identically.** Its input is `"y".repeat(30)` — a single unbroken token — and
   `wrap_words` breaks overlong tokens too (that is
   `wrap_words_breaks_an_overlong_token`'s requirement). Hard-wrap and
   word-wrap both yield 3×10 rows of `y`, so the assertion cannot distinguish
   them.

## What should happen

Both properties are asserted **through `layout_blocks`** — the entry point the
viewer actually uses — with fixtures whose word-wrapped and hard-wrapped
results differ.

`wrap_line_hard` (`src/cli/render.rs`) emits a row every `width` visible
characters, so for ordinary words the two strategies diverge:

- `"aaa bbb ccc"` at width 5 → hard: `["aaa b", "bb cc", "c"]`; word:
  `["aaa", "bbb", "ccc"]`.
- `"aaa bbb ccc ddd"` at width 7 → hard: `["aaa bbb", " ccc dd", "d"]`; word:
  `["aaa bbb", "ccc ddd"]`.

## Root cause

**Architect-side, and the second occurrence of one shape in this phase.** The
round-2 criteria named a test after the helper (`wrap_words_...`) rather than
after the behaviour, and specified an output fixture that cannot separate the
two wrappers. Phase-03's round-1 review already recorded a tautological test of
the same family — `expanded_layout_is_unchanged_by_the_new_path`, which
compared a wrapper with the function it delegates to.

The shape: **a criterion that names a function tends to produce a test of that
function; only a criterion that names an observable behaviour produces a test
of the wiring.**

## Definition of done

The two existing weak tests are replaced or supplemented so that each of the
following holds. Every item was checked against the current tree.

- [ ] Test `layout_wraps_prose_on_word_boundaries` passes — via
      `layout_blocks(&[Block::Assistant { text: "aaa bbb ccc ddd".into() }], 7)`,
      the `Assistant` rows are exactly `["aaa bbb", "ccc ddd"]`. (Currently
      absent.)
- [ ] Test `layout_keeps_output_hard_wrapped` passes — via
      `layout_blocks(&[output_block("aaa bbb ccc", 0)], 5)`, the `Output` rows
      are exactly `["aaa b", "bb cc", "c"]`. (Currently absent.)
- [ ] **Mutation M2 demonstrates both guards bite.** Add a second mutation pair
      to the phase's E2E block, targeting the *wiring*, not a helper:
      - apply: at the `Block::Output` arm, change `push_wrapped_hard` to
        `push_wrapped`; run `cargo test --lib cli::viewer`; it **must fail**
        `layout_keeps_output_hard_wrapped`.
      - restore, then apply: at the `UserTurn` and `Assistant` arms, change
        `push_wrapped` to `push_wrapped_hard`; it **must fail**
        `layout_wraps_prose_on_word_boundaries`.
      - restore; suite green; `git status --short` clean.
      Record both directions in the artifact with `grep -c` before/after, as
      the existing M1 pair does.
- [ ] The four round-2 criteria still hold: `grep -c "Modifier::UNDERLINED"
      src/cli/viewer.rs` = 0, and `style_for_focused_is_distinct_without_underline`,
      `wrap_words_breaks_an_overlong_token`,
      `layout_blocks_renders_full_output`,
      `collapsed_output_lays_out_as_exactly_one_row` all pass.
- [ ] All four gates green.

## Out of scope

- **Changing the shipped wrap or focus behaviour.** Round 2 got the behaviour
  right; this bug is only about the tests that guard it. If a fixture reveals a
  genuine behavioural difference, stop and report rather than adjusting the
  behaviour to match the test.
- `wrap_line_hard` itself, and the inline chat surface.
