# Phase 04 — top-attach

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-02
**Depends on:** phase 03 (done)
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Design item 4 (`docs/design/2.7.0-UX-improvements.md`): in the live
`daemoneye top`, the user picks a shell in the Shells panel and watches it
live and read-only, then returns to the panels as they were. PE decision 7
fixes the mechanism: the view reuses the `/tail` model (`TailModel`,
`render_tail`) and the `ShellTail` stream, and no key sends input or a
signal to the shell. FINDINGS **P5** rides along: the disk refresh moves
off the key-handling path, which the attach view shares.

## Architecture references

- `src/cli/commands/top_tui.rs` · `TopModel`, `handle_key`, `run_tui`,
  `plan_refresh`, `shells_refresh`; `top_tui_draw.rs` · `draw`, the footer.
- `src/cli/commands/top_load.rs` · `disk_panels`, `fingerprint`.
- `src/cli/tail.rs` · `TailModel`, `TailAction`, `tail_key_action`,
  `TailModel::act` / `apply`; `src/cli/tail_view.rs` · `open_tail`,
  `run_tail_viewer` (how it opens the `ShellTail` connection and reads frames
  with the cancel-safe `lines().next_line()`), `tail_loop`, `render_tail`.
- `docs/dev/milestones/M36-ux-and-audit/FINDINGS.md` row P5.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the files above, and the `daemoneye top` paragraph and table rows in
   `README.md`.
3. Read this entire phase doc before touching any code.

## Behaviour

**Shells selection.** With Shells focused, `↑↓` / `k`·`j` / `PageUp` /
`PageDown` / `Home` / `End` move a selected shell row — drawn reversed, as
the selected run is — and the panel's window follows the selection (today
they scroll the rows). A shells refresh keeps the selection on the same
shell id; when that shell is gone it clamps into the new list. With no rows
(`none`, `daemon not running`) the keys do nothing.

**Opening.** With Shells focused and a shell selected, `Enter` or `a` opens
the attach view on it — not while the filter is being edited, and not with
another panel focused. The footer names `enter attach` while Shells is
focused and has rows. Opening sends `Request::ShellTail { id, follow: true
}` on its own daemon connection and builds the model from the first frame
as `open_tail` does. If the connection or the first frame fails (the daemon
is down, the shell is gone), the view does not open: the panels stay and the
footer shows the error until the next key.

**The view.** It takes the whole screen, drawn by `render_tail` — the
shell's grid at the shell's own size and the `tail …` status line. Frames
keep arriving while it is open; when the stream ends (the shell exited, the
daemon went away) the model is marked exited and the view stays until
closed. Keys mean what they mean in `/tail` (`tail_key_action`): scroll,
`f` follow, `/` search with `n`/`N`, and `q` / `Esc` / `Ctrl-O` close the
view — **back to the panels exactly as they were**: focus, selections and
offsets. `Ctrl-C` quits `top` in every mode, as today. While the view is
open the panels keep refreshing underneath, so they are current on return.

**Read-only.** The view sends nothing but the one `ShellTail` request; no
key reaches the shell as input, a signal or a resize.

**P5 — the disk refresh leaves the key path.** `disk_panels` runs on a
blocking thread, not on the task that handles keys, and its result is
applied when it arrives; at most one disk refresh is in flight at a time,
and keys are handled and drawn while one runs.

## Constraints

- **Reuse, don't copy:** the view draws with `render_tail` and decides keys
  through `tail_key_action` (over a translation of crossterm's `KeyEvent` to
  `cli::input::Key`) and `TailModel::act`. No second key table, no second
  grid painter. `tail_loop` itself does not fit — it owns its own key source
  — so `top`'s loop drives the model directly.
- **A child module:** `top_tui.rs` is near the 800-line production cap, so
  the attach state, its key translation and its opening live in a new child
  module of `top_tui.rs` (as `top_tui_draw.rs` is), with its own inline
  tests. `run_tui` gains only the select arm for the view's frames and the
  arm for the disk result. The allowlist is a ratchet: if an allowlisted
  function grows, raise its line and say why; do not squeeze.
- **Testable without a terminal:** the view's state over a generic frame
  source (as `tail_loop` takes `tokio::io::Lines<R>`), so a test drives it
  with a duplex; drawing pinned on a `TestBackend`.
- **Docs follow:** README's `daemoneye top` paragraph and its live-view
  table row describe the Shells selection, `Enter`/`a`, the view and its
  keys; CLAUDE.md's key-files table gets a row for the new module.
- **Use `patch` with whole blocks** on long files, never `patch_lines`.

## Spec

1. **Red tests first** — write these against the current code, stubbing new
   functions where needed, run them, and keep the failing output for step 6
   (the `top_attach_` prefix matches no test today):
   - `top_attach_shells_selection_moves_and_survives_refresh` — the keys
     move it and the window follows; a refresh keeps the same id, clamps
     when it is gone; empty panel ignores the keys.
   - `top_attach_enter_or_a_opens_only_on_a_selected_shell` — `Enter` and
     `a` with Shells focused name the selected id; Runs focused, no rows, or
     editing the filter name nothing.
   - `top_attach_keys_close_on_q_esc_ctrl_o_and_ctrl_c_quits_top` — the
     translated keys give `tail_key_action`'s actions, including search
     typing; `Ctrl-C` sets `quit`.
   - `top_attach_view_draws_the_shell_and_close_restores_the_panels` — on a
     `TestBackend`: the grid's text and the `tail <id>` status line while
     open, none of the panel titles; after `Esc` the four panels with the
     same focus, selection and offsets.
   - `top_attach_stream_end_marks_exited_and_keeps_the_view` — driven by a
     duplex: a frame updates the grid, closing the writer marks it exited,
     the view stays open.
   - `top_attach_open_error_leaves_the_panels_and_names_it` — an error first
     frame opens nothing and the footer shows the message; the next key
     clears it.
   - `plan_refresh_holds_disk_while_one_is_in_flight` — with a disk refresh
     in flight, a changed fingerprint or an aged snapshot asks for none.
2. **Selection and opening** — the Shells selection, the open trigger, the
   footer hint and the error note on `TopModel`.
3. **The view** — the child module: state, key translation, opening over
   the IPC client, and the `run_tui` arms; drawing in `draw`.
4. **P5** — `disk_panels` off the key task with the one-in-flight rule;
   mark FINDINGS P5 `fixed: M36 phase-04,
   plan_refresh_holds_disk_while_one_is_in_flight`.
5. **Docs** — per the constraint. `cargo test` passes with `failed=0`.
6. **Red runs** — one `### Update — <date> (red runs)` entry pasting step
   1's `test result:` lines and assertion messages, copied verbatim from the
   run's output (no elisions or annotations inside the fence).
7. **Findings** — one `### Findings — <date>` entry: anything you noticed
   outside this phase's scope, two or three lines, or `none`.
8. **End-to-end** — run the block in § End-to-end verification once before
   committing and paste its output, fenced, in an
   `### Update — <date> (end-to-end verification)` entry. Required.
9. **Commit** as `feat: top attaches read-only to a shell from the Shells panel; the disk refresh leaves the key path (M36 phase-04)`.

## Acceptance criteria

- [ ] The E2E block's first line is `top_attach passed=6 failed=0` (today
      `top_attach passed=0 failed=0`) and its second
      `plan_refresh_holds passed=1 failed=0` (today `passed=0`).
- [ ] The E2E block's `readonly_hits=0` line holds over every
      `src/cli/commands/top*.rs` file (preservation: today 0, and the new
      module must keep it).
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] The existing `top_tui` tests pass (preservation) — in particular
      `enter_opens_the_selected_runs_detail`,
      `esc_restores_focus_selection_and_offsets`,
      `shells_scroll_clamps_at_both_ends` (updated only as far as the
      Shells keys now move a selection) and the `tail_loop_*` tests.
- [ ] FINDINGS P5's triage cell reads `fixed: M36 phase-04, …`.
- [ ] README's `daemoneye top` paragraph names `Enter`/`a` on a shell and
      the read-only view; CLAUDE.md names the new module.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0`, and `failed=0`.

## Test plan

Pure tests pin the selection, the trigger, the key translation and the
refresh plan; a duplex-driven test pins the stream; `TestBackend` pins what
the view draws and what closing restores. The milestone's live check opens a
shell from `top` on pinky, watches it update, and returns with `q`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test top_attach_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "top_attach passed=" p, "failed=" f}'
cargo test plan_refresh_holds 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "plan_refresh_holds passed=" p, "failed=" f}'
cat src/cli/commands/top*.rs | grep -cE 'ShellInput|ShellSignal|ShellResize' | sed 's/^/readonly_hits=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/top_tui.rs`, `top_tui_draw.rs`, `top_load.rs`,
  `top.rs` (only if `run_tui`'s caller must change), `src/cli/tail.rs` and
  `src/cli/tail_view.rs` (only to expose what the view reuses — no change to
  `/tail`'s behaviour), `README.md`, `CLAUDE.md`,
  `docs/dev/milestones/M36-ux-and-audit/FINDINGS.md` (row P5), and
  `tests/hygiene_allowlist.txt` (ratchet only).
- Create one child module of `top_tui.rs` for the attach view.

## Out of scope

- Typing into the shell from `top` (that is `/attach` in chat), pausing or
  signalling it, resizing it to the terminal.
- Opening a shell's recording (`/log`) or a dedicated/exited shell's
  history from `top`.
- `top --once` output and the other FINDINGS rows.

## Update Log

<!-- entries appended below this line -->
