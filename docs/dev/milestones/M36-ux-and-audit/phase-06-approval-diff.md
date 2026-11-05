# Phase 06 — approval-diff

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-04
**Depends on:** phase 05b (`wrap_line`)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bug, size=s

## Goal

Fix finding B7 (`FINDINGS.md`). A `write_script` / `write_runbook` /
`edit_file` approval shows its diff as `[32m+line[0m`: `cli/diff.rs` ·
`render_diff` returns strings with ANSI SGR codes, and
`stream_prompts.rs` · `prompt_write_ratatui` and `prompt_edit_file_ratatui`
send them through the plain `RatatuiRenderer::commit`. `set_string`
drops only the ESC byte, so the rest of each code shows up as text. `commit`
also clips each line at the terminal width with `…`, and a new file shows
only its first 80 lines. Users approve content they cannot see.

## Architecture references

- `src/cli/diff.rs` · `render_diff` (only those two prompts call it).
- `src/cli/render_ratatui_draw.rs` · `commit` (plain, clips) and
  `commit_styled` (styled spans).
- `src/cli/render.rs` · `wrap_line`, the span-level splitter that keeps
  styles (phase 05b).
- `src/cli/commands/stream_prompts.rs` · `read_approval_input_with`,
  where the approval prompt decides on its first key, and
  `prompt_with_session_approve`.
- `src/cli/viewer.rs` · `run_transcript_viewer`, the Ctrl+O viewer.
  `src/cli/transcript.rs` · `Transcript`, `Block`.

## Behaviour

**Styled, never escaped.** Each diff row reaches scrollback as styled spans,
and no committed row contains ESC, `[32m` or any other SGR text. Use the
same colours as today:
- the `new file:` / `modified:` header is bold;
- `+` lines are green and `-` lines are red;
- `@@` hunk headers are cyan;
- context lines are unstyled;
- the `… more lines` footer and `no changes:` are dim.

**Wrapped, not clipped.** A diff line wider than the terminal continues
on the next row, keeping its style, with no character dropped and no `…`.
Wrap at commit time against the renderer's width.

**Counts in the header.**
- A new file's header shows its line count, e.g. `new file: x.sh (120 lines)`.
- A modified file's header shows the lines added and removed, e.g.
  `modified: x.sh (+5 -3)`.
- `no changes:` is unchanged.

**The full diff is one key away.** At these two approval prompts, Ctrl+O
(byte `0x0f`) as the first key opens the transcript viewer on the whole
diff. The viewer is not capped at 80 lines, and the diff opens expanded,
not collapsed. Leaving the viewer redraws the same prompt, which keeps
waiting against its original deadline. Ctrl+O is not an answer: it
neither approves nor denies, and it does not start a typed message.

When the new-file cap clips the inline diff, the footer names the key,
e.g. `… (40 more lines — ctrl+o to view all)`. The cap stays at 80
lines inline.

## Constraints

- Do the first-key decision as a pure function that the tests call
  directly. `AsyncStdin` has no test seam.
- Every caller of `read_approval_input_with` (the command panel, the
  credential and yes/no prompts) keeps its current behaviour for every key
  other than Ctrl+O. Ctrl+O at those other prompts is ignored and opens
  nothing.
- The viewer runs on a transcript that holds only the diff. Do not thread
  the chat's `Transcript` into the prompt functions. A SIGWINCH stream
  may be created locally (`tokio::signal::unix::signal`).
- Preservation: the existing `diff.rs` tests keep their meaning. They may
  be adapted from ANSI strings to spans. The `parse_approval_decision_*`
  tests and `approval_read_times_out` stay green without being edited.

## Spec

1. **Red tests first.** Write these against the current tree and run them.
   Keep the failing output for step 4. All carry the `approval_diff_`
   prefix:
   - `approval_diff_rows_carry_styles_not_escapes`: for a new file and for
     a modified file, no row's text contains `\x1b` or `[3`. The `+` rows
     are green, `-` red, `@@` cyan, and the header bold.
   - `approval_diff_header_counts_lines`: a 120-line new file's header
     shows `120 lines`, and a change adding 2 and removing 1 shows `+2 -1`.
   - `approval_diff_long_line_wraps_at_commit`: a 200-char `+` line
     committed at width 80 fills more than one row of the renderer's
     history. Every row is green, no row holds `…`, and the rows' text
     joins back to the line. Use the test renderer that the
     `render_ratatui_tests.rs` approval tests use.
   - `approval_diff_clipped_footer_names_key`: a 120-line new file shows
     80 `+` rows and a footer naming `ctrl+o`. A 10-line file has no
     footer.
   - `approval_diff_full_is_uncapped`: the content the viewer gets for the
     120-line file holds all 120 lines.
   - `approval_diff_ctrl_o_is_not_an_answer`: the first-key function maps
     `0x0f` to a view request when viewing is offered, and to "ignore" when
     it is not. `y`, `n`, `a`, Enter and a letter keep today's outcomes.
2. **Implement** per § Behaviour.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add one `### Update — <date> (red runs)` entry. Paste
   step 1's `test result:` lines and assertion messages verbatim, with
   nothing elided.
5. **Findings.** Add one `### Findings — <date>` entry, two or three lines,
   or `none`.
6. **End-to-end.** Run § End-to-end verification once. Paste its output,
   fenced, in an `### Update — <date> (end-to-end verification)` entry.
7. **Commit** as `fix: write and edit approvals show a styled, wrapped diff with the full content behind ctrl+o (M36 phase-06)`.

## Acceptance criteria

- [ ] The E2E block's first line reads `approval_diff passed=P failed=0`
      with P ≥ 6. Today it reads `passed=0`.
- [ ] The E2E block shows `diff_commit_plain=0`. No diff row goes through
      plain `commit` in `stream_prompts.rs`. Today the count is 2.
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] Preservation: the `diff.rs` tests, the `parse_approval_decision_*`
      tests and `approval_read_times_out` pass.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test approval_diff_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "approval_diff passed=" p, "failed=" f}'
grep -cE 'renderer\.commit\(&format!\("  \{\}\\n", line\)\)' src/cli/commands/stream_prompts.rs | sed 's/^/diff_commit_plain=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/diff.rs`, `src/cli/commands/stream_prompts.rs`,
  `src/cli/render_ratatui_draw.rs`, `src/cli/render_ratatui_tests.rs`,
  `src/cli/commands/stream.rs` (tests only) and `src/cli/viewer.rs` /
  `src/cli/transcript.rs`, only if opening the viewer on a one-diff
  transcript needs a small constructor or entry point.
- A new or raised `tests/hygiene_allowlist.txt` entry is acceptable if a
  touched file or function crosses its cap. State the reason in the
  Findings entry.

## Out of scope

- The clipping that plain `commit` does on other lines, such as a long
  path in the `⚙ AI wants to write` header line.
- Capping a modified file's unified diff. Today it is uncapped, and it
  stays uncapped.
- Raw ESC in model text (`parse_ansi_to_spans`).
- The command approval panel (phase 05a).

## Update Log

<!-- entries appended below this line -->
