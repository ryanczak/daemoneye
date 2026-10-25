# Phase 10b — One padding for every panel title

**Status:** in-progress
**Milestone:** M30 — Telemetry & dashboard
**Drafted:** 2026-10-25 (from the L8 rehearsal and PE run, finding F1)

## Goal

Every bordered title in `daemoneye top` reads `╭ <title> ─`, one space on
each side. Today Runs, Shells, Activity and the detail view read
`╭  1 Runs  ─` while Budget reads `╭ 2 Budget ─`.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.7 (`daemoneye top`).
- `src/cli/commands/top_tui.rs` — `draw_at`, `draw_detail_at`,
  `draw_panel`, `panel_title`, `panel_title_line`.

## Pre-flight

- `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`
  and `cargo test` are green at HEAD.
- `screen_draws_the_four_panels` (`top_tui.rs` tests) shows how a screen is
  rendered into a `TestBackend` and read back as text;
  `enter_opens_the_selected_runs_detail` shows how a detail view is opened
  on a model.

## Current state

`panel_title` returns a title already padded with one space on each side,
and `draw_panel` pads its `title` argument again. Budget does not go
through `draw_panel` (it styles `panel_title_line` itself), so it alone is
single-padded. The detail view passes a pre-padded ` Run <id> ` to
`draw_panel` and is doubled too.

## Spec

1. One place pads. On screen, the four panel titles and the detail title
   are each preceded by `╭ ` and followed by ` ─`, with exactly one space
   on each side of the text, at every terminal size and whether or not the
   panel is focused.
2. No other change to what a title says (`<n> <Name>`, `Run <id>`) or how
   it is styled (bold; cyan when focused).

## Acceptance criteria

- [ ] `cargo test every_title_is_padded_once` passes (`1 passed`): a
      screen drawn at 80×24 and at 160×48 contains `╭ 1 Runs ─`,
      `╭ 2 Budget ─`, `╭ 3 Shells ─` and `╭ 4 Activity ─` as substrings of
      its rows, and contains neither `╭  ` nor `  ─` on a title row.
- [ ] `cargo test the_detail_title_is_padded_once` passes (`1 passed`):
      with a run's detail open, a row contains `╭ Run <id> ─` and no row
      contains `╭  Run`.
- [ ] Preservation: `cargo test top_tui` has no failures (the focus-colour
      and title tests of phases 12–14 among them); `cargo test` has no
      failures.

## Test plan

- Unit, `src/cli/commands/top_tui.rs`: `every_title_is_padded_once`,
  `the_detail_title_is_padded_once`. Build the rows from the
  `TestBackend` buffer as `screen_draws_the_four_panels` does, row by row,
  so `╭ 1 Runs ─` is matched within one row.
- Mutation check, stated in the Update Log: with the fix in place,
  reintroduce one extra space in the padding, confirm
  `every_title_is_padded_once` fails, restore.
- The live look is checked by the PE at the milestone close, not here.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
A=/tmp/e2e-m30-10b.txt; : > "$A"
for t in every_title_is_padded_once the_detail_title_is_padded_once; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: two `ok. 1 passed` lines, clippy finished with no warnings,
`fmt_exit=0`, `failed=0`.

## Authorizations

- Edit `src/cli/commands/top_tui.rs` only.
- Add the two tests. No new dependencies.

## Out of scope

- Any change to `src/cli/commands/top.rs`, the layout, the footer, the
  Budget bar or the detail text.
- The `top --once` plain-text output, which has no borders.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-25 11:06 (progress)

Started phase 10b (executor, headless run). Flipped the phase and the M30
README's 10b row to `in-progress`. Plan: make `draw_panel` render the title
text as given, drop the padding from `panel_title`, let `draw_at`'s four
call sites and `panel_title_line` pad once, have `draw_detail_at` pass the
bare `Run <id>`, then the two tests, the mutation check and the end-to-end
capture.

### Update — 2026-10-25 11:13 (progress)

All Spec tasks implemented in `src/cli/commands/top_tui.rs`: `draw_panel`
renders its `title` argument with the one padding (`format!(" {title} ")`),
`panel_title` returns the bare `<n> <Name>` (no padding), the Budget block's
`panel_title_line` pads once itself, and `draw_detail_at` passes the bare
`Run <id>`; doc comments updated to match. Two tests added:
`every_title_is_padded_once` (80×24 and 160×48; the four `╭ <n> <Name> ─`
needles per row, plus no `╭  ` and no `  ─` on any row) and
`the_detail_title_is_padded_once` (`╭ Run <id> ─` present, no `╭  Run`); the
phase-14 test `screen_draws_the_detail_view`'s title needle was updated to
the bordered form `╭ Run <id> ─` (it asserted the old doubled padding ` Run
<id> `). Mutation check done: an extra space in `draw_panel`'s padding made
both new tests fail (`a row contains "╭ 1 Runs ─"`, `the panel is titled
with the run id`), restored, both pass. `cargo fmt --all`, `cargo build`,
`cargo clippy --all-targets --all-features -- -D warnings` clean;
`cargo test` 2570 passed, 0 failed (`cargo test top_tui` 45 passed).
Next: the end-to-end capture entry.

### Update — 2026-10-25 11:14 (end-to-end verification)

Ran the block in § End-to-end verification verbatim, unmodified; the
artifact `/tmp/e2e-m30-10b.txt` was generated mechanically by the block
(`A=/tmp/e2e-m30-10b.txt; : > "$A"`, the two `cargo test <name>` lines, the
clippy tail, the `cargo fmt --all --check` exit and the full-suite awk) and
its contents are pasted byte-exact below.

```
every_title_is_padded_once: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
the_detail_title_is_padded_once: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2570 failed=0
```

Expected vs actual: two `ok. 1 passed` lines — present; clippy finished
with no warnings — the tail line is a bare `Finished`, no warnings;
`fmt_exit=0` — got 0; `failed=0` — got 0. Every line of the expected shape
is present.
