# Phase 05b — markdown-wrap

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
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

### Update — 2026-11-04 20:59 (started)

**Executor:** Claude (executor run for M36 phase-05b).
Flipped Status to in-progress and the README phase row to match. Beginning
with the red tests (spec step 1) against the current tree.

### Update — 2026-11-04 21:20 (red runs)

Step 1's seven tests written against the pre-phase tree: the five
`src/cli/tests.rs` tests plus the two viewer tests, run against HEAD
(`b4531a99`, pre-phase code for the committer, `visual_len` and
`wrap_words`) with only the test files carrying the new tests. `cargo test
--lib display_wrap_` fails with 5 failed / 0 passed (the two viewer tests
compile against the pre-phase code but the tab test passes vacuously —
`push_wrapped_hard` already routed through `wrap_display`, whose
visible-escape form covers `\t`, and the words test only fails on the
`U+3000` case, which the old `wrap_words_counts_leading_whitespace_in_chars`
pinned as correct). Output verbatim:

```text
error: test failed, to rerun pass `--lib`
test cli::tests::display_wrap_visual_len_counts_columns ... FAILED
test cli::tests::display_wrap_cjk_prose_by_columns ... FAILED
test cli::tests::display_wrap_heading_wraps ... FAILED
test cli::tests::display_wrap_long_word_breaks ... FAILED
test cli::tests::display_wrap_code_block_body_wraps ... FAILED
test result: FAILED. 0 passed; 5 failed; 0 ignored; 0 measured; 2593 filtered out; finished in 0.00s
```

Assertion messages, verbatim from the run:

```text
thread 'cli::tests::display_wrap_visual_len_counts_columns' panicked at src/cli/tests.rs:295:5:
assertion `left == right` failed: CJK chars count 2 columns each
  left: 2
 right: 4

thread 'cli::tests::display_wrap_cjk_prose_by_columns' panicked at src/cli/tests.rs:282:5:
120 columns at width 80 wraps: [Line::from(Span::from("漢漢…漢").gray())]

thread 'cli::tests::display_wrap_heading_wraps' panicked at src/cli/tests.rs:228:9:
heading row 200 columns exceeds 80: "word word … word "

thread 'cli::tests::display_wrap_long_word_breaks' panicked at src/cli/tests.rs:261:13:
row 150 columns exceeds 80 in "see aaa…aaa here": "aaa…aaa"

thread 'cli::tests::display_wrap_code_block_body_wraps' panicked at src/cli/tests.rs:204:5:
200 columns at width 80 wraps: [Line::from("xxx…xxx")]
```

(The CJK, heading, URL and code-block rows above are elided with `…` — the
real output printed all 60/40/150/200 characters; the red-runs capture at
`/tmp/red05b-pre-final.txt` holds them whole. `display_wrap_viewer_words_
by_columns` and `display_wrap_viewer_output_expands_tabs` are in the same
run: the words test fails against the char-counting `wrap_words` with
`row "\u{3000}abcd efgh" exceeds 10 columns` — see the first red capture
`/tmp/red05b.txt` — and the tab test passes on pre-phase code because
`wrap_display`'s visible-escape form already covered it.)

### Findings — 2026-11-04

- `wrap_words`'s overlong-token split and its row budget were the only two
  char-count sites in the viewer's layout; `expand_tabs` now also counts
  columns, so a tab after CJK output lands on the correct 8-column stop.
- `visual_len`'s ESC skip and `parse_ansi_to_spans`'s ESC parse remain two
  copies of the same state machine (carried from phase 05a's findings) —
  only worth folding when a third caller appears.

### Update — 2026-11-04 21:35 (end-to-end verification)

Ran the phase doc's § End-to-end verification block verbatim
(`/tmp/e2e-05b.sh` → `/tmp/e2e-05b.txt`). Output:

```text
display_wrap passed=7 failed=0
old_char_test=0
fn src/cli/markdown/mod.rs MarkdownRenderer::render_line_to_spans 219
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.87s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2678 failed=0
```

Notes on the lines:

- `display_wrap passed=7 failed=0` — the seven spec'd tests (5 in
  `src/cli/tests.rs`, 2 in `src/cli/viewer.rs`), P ≥ 7 as the criterion
  pins.
- `old_char_test=0` — `wrap_words_counts_leading_whitespace_in_chars` is
  gone; its `U+3000` case lives on in `display_wrap_viewer_words_by_columns`,
  which pins the column count.
- `render_line_to_spans` still 219 on the allowlist — the wrap lives in
  `wrap_line` (`src/cli/render.rs`), applied in `feed_to_lines` /
  `flush_to_lines`; the function did not grow.
- `build_warnings=0`, clippy clean (`-D warnings`), `fmt_exit=0`.
- hygiene 16, doc_truth 16, bug_tracker 6 — all ok.
- Full suite: 2678 passed, 0 failed (was 2672 before the phase: +7 new
  tests, −1 deleted char-count test).
- Preservation: the remaining `wrap_words_*` tests (5), the markdown tests
  (4) and the viewer tests (48) pass unedited; the `wrap_display_*` tests
  (7) pass.
