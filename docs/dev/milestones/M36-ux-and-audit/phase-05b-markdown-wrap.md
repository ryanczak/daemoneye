# Phase 05b — markdown-wrap

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-04
**Depends on:** phase 05a (`wrap_display`, `unicode-width`)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bug, size=s

## Goal

This phase fixes findings B8 and P6, which are about wrapping, and L6
(`FINDINGS.md`). Model output in the chat is cut off at the right edge
with no marker:
- code-block bodies and headings are never wrapped;
- a single word wider than the terminal (a URL, a path) overflows its row;
- CJK prose overflows, because `visual_len` counts chars rather than
  columns.

A long command the model suggests in a code block loses its tail in
scrollback. The transcript viewer's word wrap (`wrap_words`) counts chars
too, so CJK lines are clipped there as well. Separately, after phase 05a
the viewer's command-output rows show tabs as `\t`.

## Architecture references

- `src/cli/markdown/mod.rs` · `feed_to_lines` and `flush_to_lines`, the
  committer's only two entry points (called from `stream_ask.rs`). Both
  return what `render_line_to_spans` builds.
- `src/cli/render.rs` · `visual_len`, and `wrap_display` from phase 05a.
- `src/cli/viewer_layout.rs` · `wrap_words`, and `push_wrapped_hard`
  (machine-output rows).

## Behaviour

**The committer never returns a row wider than its width.** This holds for
every `Line` that `feed_to_lines` and `flush_to_lines` return: code-block
bodies, fences, headings, prose, list items, blockquotes and rules alike.
Width is measured in display columns, so `漢` counts 2 and a styled span
counts only its text. A row that is too wide splits into more rows:
- text and styles are preserved, so a continuation row keeps the style of
  the text it carries (a heading's second row is still a bold heading, a
  code row keeps its highlighting);
- a wide character is never split across the edge;
- no character is dropped.

Word wrapping of prose and lists still breaks at spaces where it can. Only
a single word wider than the row is split inside the word.

**`visual_len` measures display columns.** It still skips the ANSI
sequences the renderer inserts itself.

**The viewer.** `wrap_words` measures display columns, both for the row
budget and for its overlong-token split. `push_wrapped_hard` expands each
tab to the next 8-column stop before calling `wrap_display`. Every other
control character stays visible, as phase 05a made it.

## Constraints

- Keep the split in one span-level helper, for example
  `wrap_line(line: Line<'static>, width) -> Vec<Line<'static>>` in
  `src/cli/render.rs`, applied in `feed_to_lines` and `flush_to_lines`.
  `render_line_to_spans` is on the hygiene allowlist at 219 lines and must
  not grow. Put no wrapping inside its branches.
- Do not route the committer through `wrap_display`. Its visible-escape
  rule is for the approval panel, and here the spans are already parsed.
- `wrap_words_counts_leading_whitespace_in_chars` pins the old char count:
  `U+3000` is 2 columns wide. Replace it with
  `display_wrap_viewer_words_by_columns`, which pins the column count.
- Preservation: the other `wrap_words_*` tests, the markdown tests and the
  viewer tests stay green without being edited.

## Spec

1. **Red tests first.** Write these against the current tree, run them, and
   keep the failing output for step 4. All carry the `display_wrap_`
   prefix:
   - `display_wrap_code_block_body_wraps`: a 200-char line inside a
     ```` ```bash ```` block at width 80. Every row is at most 80 columns,
     and the rows' text joins back to the line.
   - `display_wrap_heading_wraps`: a long `## ` heading. Every row fits,
     and every row of the heading is bold.
   - `display_wrap_long_word_breaks`: a 150-char URL in prose, in a `- `
     item, in a `1. ` item and in a `> ` quote. Every row fits, and the URL
     is intact across the rows.
   - `display_wrap_cjk_prose_by_columns`: 60 CJK chars of prose (120
     columns) at width 80. Every row fits.
   - `display_wrap_visual_len_counts_columns`: `visual_len("漢字") == 4`,
     and an SGR-wrapped word measures as its text.
   - `display_wrap_viewer_words_by_columns`: `wrap_words` on CJK words at
     width 10 gives no row wider than 10 columns, including the `U+3000`
     case that replaces the old test.
   - `display_wrap_viewer_output_expands_tabs`: an output row `a\tb` shows
     `a` and then `b` at column 8, with no `\t`. A CR still shows as `\r`.
2. **The helper, `visual_len` and the viewer**, per § Behaviour.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add one `### Update — <date> (red runs)` entry that pastes
   step 1's `test result:` lines and assertion messages verbatim from the
   run's output.
5. **Findings.** Add one `### Findings — <date>` entry, two or three lines,
   or `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its output,
   fenced, in an `### Update — <date> (end-to-end verification)` entry.
7. **Commit** as `fix: chat markdown and the viewer wrap by display width instead of clipping (M36 phase-05b)`.

## Acceptance criteria

- [ ] The E2E block's first line reads `display_wrap passed=P failed=0`
      with P ≥ 7 (today `passed=0`).
- [ ] The E2E block shows `old_char_test=0`:
      `wrap_words_counts_leading_whitespace_in_chars` is gone (today 1).
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] Preservation: the other `wrap_words_*`, markdown, viewer and
      `wrap_display_*` tests pass.
- [ ] The allowlist line for `render_line_to_spans` is unchanged at 219.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test display_wrap_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "display_wrap passed=" p, "failed=" f}'
grep -rn 'wrap_words_counts_leading_whitespace_in_chars' src/ | wc -l | sed 's/^/old_char_test=/'
grep 'render_line_to_spans' tests/hygiene_allowlist.txt
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/render.rs`, `src/cli/markdown/mod.rs` (`feed_to_lines`,
  `flush_to_lines` and its tests only), `src/cli/viewer_layout.rs`,
  `src/cli/viewer.rs` (its `wrap_words_*` tests) and `src/cli/tests.rs`.

## Out of scope

- The approval panel (phase 05a) and diff styling (phase 06).
- Raw ESC bytes inside model text. `parse_ansi_to_spans` still interprets
  them as styling. That is a separate question, not a wrapping one.
- P6's per-keypress relayout (deferred; no lag has been observed).

## Update Log

<!-- entries appended below this line -->
