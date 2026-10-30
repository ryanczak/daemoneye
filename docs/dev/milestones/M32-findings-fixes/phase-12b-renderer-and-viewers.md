# Phase 12b — Renderer and viewers

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-12
**Estimated diff:** ~115 insertions, ~145 deletions
**Tags:** language=rust, kind=refactor, size=s
**Findings:** M31 `FINDINGS.md` #27, #30, #55, #56 (#28 is rejected
below)

## Goal

The chat renderer and the full-screen viewers repeat their setup code.
After this phase:

- **#55:** one `render_status_bar` in `src/cli/render_ratatui.rs` draws
  the dim ` session:… · model · up … ` line. It replaces six copies:
  three in the live-region renderers and three in the panel methods of
  `render_ratatui_draw.rs`.
- **#56:** one `alert_panel` builds the red-bordered shell the approval
  and credential dialogs share. The picker keeps its plain border: it
  asks for a choice, not an approval, and the helper's doc says so.
- **#27:** `src/cli/viewer.rs` gains `enter_alt_screen` and
  `resize_events`. The `/tail`, `/log` and attach viewers call them
  instead of each entering the alternate screen, building the teardown
  guard, the terminal and the resize stream by hand.
- **#30:** a new test, `render_tail_keeps_the_log_status_line_while_a_note_is_set`,
  pins one ordering in `render_tail`'s status match that no test held.
  The other load-bearing ordering, replay before log, is already pinned
  by `render_tail_shows_the_replay_status_line`.

No behaviour changes. The suite goes from **2609** to **exactly 2610**:
one test added, none removed.

**Rejected:** **#28**, the empty reader `run_log_viewer` passes to
`tail_loop` as "no frame source". The function's doc comment names it,
`tail_loop`'s contract makes a closed stream mean exactly that, and the
log tests drive the loop the same way. An `Option` would add a branch to
the loop for one caller.

## Architecture references

- `src/cli/viewer.rs` § `AltScreenGuard`: the guard the new entry
  returns.
- M32 README, decision 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-12b-renderer-and-viewers.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `840ee61c`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2610 failed=0`. It asserts its own anchors and prints three
   lines, the last
   `render_tail_keeps_the_log_status_line_while_a_note_is_set added`. If
   an assert fires, stop and report it in the Update Log. Do not
   hand-edit anything it touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `refactor:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-12b-renderer-and-viewers.py`
(sha256 begins `16f70f90e8e05d60`), kept in the repo as the record of the change.

### The red run is done — do not repeat it

The new test was run red at drafting, with the log arm moved below the
note-carrying tail arm. A log view stepped to a command then showed the
tail's status line:

```
"tail s3-17 · 5×20 · exited · command 1/3 · exit 0 · cmd-0 · n/N older/newer · esc to close"
```

and the test failed on `bottom.starts_with("log s3-17")`. With the arm
order as it stands, it passes.

## Acceptance criteria

- [ ] The End-to-end block prints
      `status_bars=1 alert_shells=0 alt_entries=0 order_test=1` (today:
      `status_bars=6 alert_shells=2 alt_entries=6 order_test=0`).
- [ ] `cargo test` reports **exactly** `passed=2610 failed=0` (today
      2609).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "status_bars=$(cat src/cli/render_ratatui.rs src/cli/render_ratatui_draw.rs | grep -c '" session:{} · {} · up {} ",') alert_shells=$(grep -c '.border_style(Style::default().fg(red))' src/cli/render_ratatui_draw.rs) alt_entries=$(cat src/cli/tail_view.rs src/cli/attach.rs | grep -c 'EnterAlternateScreen') order_test=$(grep -c 'fn render_tail_keeps_the_log_status_line_while_a_note_is_set' src/cli/tail.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `status_bars=1 alert_shells=0 alt_entries=0 order_test=1`,
`allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker
`ok. 6 passed`, clippy clean, `fmt_exit=0`, `passed=2610 failed=0`. If
the suite stops early on a failure, paste that output, run the block once
more and paste both.

## Authorizations

- Run `scripts/phase-12b-renderer-and-viewers.py`; do not edit it.
- Edit the six files it names, through the script only:
  `src/cli/render_ratatui.rs`, `src/cli/render_ratatui_draw.rs`,
  `src/cli/viewer.rs`, `src/cli/tail_view.rs`, `src/cli/attach.rs`,
  `src/cli/tail.rs`.

## Out of scope

- #28 (rejected above).
- `run_transcript_viewer` and `daemoneye top`, which enter the
  alternate screen with mouse capture or raw mode and so differ from the
  three viewers this phase joins.

## Update Log

<!-- entries appended below this line -->
