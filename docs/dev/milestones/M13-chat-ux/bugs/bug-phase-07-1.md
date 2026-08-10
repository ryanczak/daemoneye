# Bug 1 on phase-07: Stale duplicate doc comment contradicts `repin_rows`

**Severity:** minor
**Status:** verified
**Filed:** 2026-08-10

## What's wrong

`src/cli/render_ratatui.rs:154-161` — the phase-06 doc comment for
`repin_rows` was left in place when the phase-07 replacement block was added
below it, so the function now carries **two** stacked doc comments (grep
`'/// Rows for a bottom repin'` returns 2). The retained older block's final
sentence — "`clear_from` wipes from the old viewport top or the new one,
whichever is higher on screen" — describes the two-arg behavior and
contradicts the three-arg code directly beneath it, which also considers
`content_end`.

## What should happen

Exactly one doc comment on `repin_rows`: the phase-07 block (the one
beginning at the *second* `/// Rows for a bottom repin:` line, `:162`,
which documents `content_end` and the park clamp). The stale block at
`:154-161` is deleted. No code changes.

## Root cause

The spec's Task 2 said "Replace the two-arg form" and supplied the full new
doc + fn; the executor's patch inserted the new doc above the fn but its
old_str anchored only on the `fn` line, leaving the previous doc block
orphaned above the insertion point.

## Definition of done

- [x] `grep -c '/// Rows for a bottom repin' src/cli/render_ratatui.rs`
      prints `1`. (Verified at round-2 review.) (Run 2026-08-10 against the current tree: prints `2` —
      confirmed failing.)
- [x] The surviving comment mentions `content_end` (grep
      `'the end of real committed content'` still returns 1). (Verified.)
- [x] Four gates green; `cargo test` still reports **1234** lib tests, not
      1235 — nothing added, only 8 comment lines deleted. (Verified.)
