# Phase 10b — One padding for every panel title

**Status:** todo
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
