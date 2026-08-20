# Bug 1 on phase-03: the focused block is underlined in full, and prose wraps mid-word

**Severity:** minor (cosmetic, but pervasive — it makes the viewer look broken)
**Status:** resolved (round 2, `dae5f5f`) — behaviour fixed; guards addressed by bug-phase-03-2
**Filed:** 2026-08-20
**Found by:** user screenshot during M17 close-out, confirmed by SGR capture in
an isolated `tmux -L de-m17c` server.

## What's wrong

### 1. Whole-block underline

`src/cli/viewer.rs`:

```rust
fn style_for_focused(kind: RowKind, palette: crate::cli::palette::Palette) -> Style {
    style_for(kind, palette).add_modifier(Modifier::UNDERLINED)
}
```

`render_transcript` applies this to **every row whose `block == focus`**, and
`viewer_loop` opens with `focus` on the **last** block. So the entire trailing
answer — every wrapped row of it — renders underlined the moment the viewer
opens. On a long reply that is dozens of underlined lines, which reads as a
rendering fault rather than as focus.

Confirmed by capturing with escapes (`tmux capture-pane -e`): eight rows of the
last block each carry `ESC[4m`, e.g.

```
^[[4m^[[38;5;15mHey Matt! ...^[[0m
^[[4m^[[38;5;15m... Everything on my^[[0m
```

Underline as a focus cue is fine for a *single* header row. It is wrong applied
to an entire multi-row block.

### 2. Prose wraps mid-word

`push_wrapped` uses `crate::cli::render::wrap_line_hard`, a hard character wrap:

```rust
fn push_wrapped(rows: &mut Vec<ViewRow>, text: &str, width: usize, kind: RowKind, idx: usize) {
    for line in crate::cli::render::wrap_line_hard(text, width) {
```

That is correct for the inline panel, where output is fixed-width machine text
and a hard cut is honest. In the viewer it is the primary reading surface, and
it splits words: the user's screenshot shows `` `/var/lo `` / `` g` `` and
`daem` / `on dir)` on consecutive rows.

Tool **output** rows should keep the hard wrap — machine output must not be
re-flowed. Prose rows (`User`, `Assistant`, `System`, `Tool` summaries) should
wrap at word boundaries.

## What should happen

1. A focused block is distinguishable **without** underlining its whole body —
   e.g. a marker or emphasis on its header row only, or a dim/bright contrast.
   The choice is the executor's; the constraint is that focus must be visible
   and must not apply `UNDERLINED` to body rows.
2. Prose wraps on word boundaries; a single token longer than the width still
   breaks rather than overflowing. `RowKind::Output` rows keep `wrap_line_hard`
   unchanged.

## Root cause

Phase-03 task 5 said "Rows whose `block == focus` render with an emphasised
style — pick it from the existing `Palette`; **nothing about the colour is
pinned**." That left the *scope* of the emphasis unpinned too, and the executor
reasonably applied it per-row. The spec should have said the emphasis applies to
the header row, or that it must not be a full-body text decoration.

The wrap is inherited from phase-02, which reused the inline panel's helper
without distinguishing prose from machine output — a reasonable first pass when
the viewer was read-only, and wrong once it became the place people read long
answers.

Both are **architect-side spec gaps**, not executor errors: each phase
implemented what its spec said.

## Definition of done

Each command below **fails against the current tree** (verified 2026-08-20) and
must pass:

- [ ] `grep -c "Modifier::UNDERLINED" src/cli/viewer.rs` prints `0`.
      (Currently `2`.)
- [ ] Test `style_for_focused_is_distinct_without_underline` passes — asserts
      the focused style **differs** from the unfocused style for the same
      `RowKind` (so focus is still visible) **and** that its
      `add_modifier` set does not contain `Modifier::UNDERLINED`.
      (Currently absent.)
- [ ] Test `wrap_words_does_not_split_words` passes — wrapping
      `"the quick brown fox jumps over the lazy dog"` at width 12 yields rows
      whose concatenation with single spaces reproduces the input, and **no row
      ends mid-word** (assert each row is a whole-word prefix). (Currently
      absent.)
- [ ] Test `wrap_words_breaks_an_overlong_token` passes — a single 30-character
      token at width 10 still yields 3 rows, none longer than 10. The negative
      case that stops the fix from simply never breaking. (Currently absent.)
- [ ] Test `output_rows_keep_hard_wrap` passes — a `Block::Output` whose `full`
      contains a 30-character unbroken token at width 10 still produces
      hard-wrapped rows of exactly 10, i.e. machine output is **not** re-flowed
      on word boundaries. (Currently absent.)
- [ ] `layout_blocks_renders_full_output` and
      `collapsed_output_lays_out_as_exactly_one_row` still pass — the row-count
      guarantees phase-03 established are unchanged for `Output` blocks.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Out of scope

- **Rendering markdown in the viewer.** The screenshot also shows literal
  `**bold**` and backticks, because phase-01 deliberately stores the raw token
  stream (lossless) and the viewer prints it plainly. Changing that is a design
  decision about whether the viewer re-renders markdown — it belongs in a
  milestone discussion, not in this bug.
- Any change to the inline chat surface, to `wrap_line_hard` itself (other
  callers depend on it), or to the viewer's key handling.
