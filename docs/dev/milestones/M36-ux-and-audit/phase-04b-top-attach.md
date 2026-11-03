# Phase 04b — top-attach

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-02 (split and refined 2026-11-03)
**Depends on:** phase 04a (the Shells selection)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

The second half of design item 4 (`docs/design/2.7.0-UX-improvements.md`):
in the live `daemoneye top`, the user opens the shell selected in the
Shells panel and watches it live and read-only, then returns to the panels
as they were. PE decision 7 fixes the mechanism: the view reuses the
`/tail` model (`TailModel`, `render_tail`) and the `ShellTail` stream, and
no key sends input or a signal to the shell.

## Architecture references

- `src/cli/commands/top_tui.rs` · `TopModel` (its Shells selection from
  phase 04a), `handle_key`, `run_tui` (its crossterm reader thread and
  channel, and phase 04a's disk-refresh channel); `top_tui_draw.rs` ·
  `draw`, the footer.
- `src/cli/tail.rs` · `TailModel`, `tail_key_action`, `TailModel::act` /
  `apply`; `src/cli/tail_view.rs` · `open_tail`, `run_tail_viewer` (how
  it opens the `ShellTail` connection), `render_tail`.
- `src/cli/commands/ipc_client.rs` · `connect`, `send_request`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, then the files above **once each**, then
   this doc. The design below is decided; do not re-derive it. Start Spec
   step 1 as soon as you have read them.

## Behaviour

**Opening.** With Shells focused and a shell selected, `Enter` or `a`
opens the attach view on it — not while the filter is being edited, and
not with another panel focused. The footer names `enter attach` while
Shells is focused and has rows. Opening sends `Request::ShellTail { id,
follow: true }` on its own daemon connection and builds the model from the
first frame as `open_tail` does. If the connection or the first frame fails
(the daemon is down, the shell is gone), the view does not open: the panels
stay and the footer shows the error until the next key.

**The view.** It takes the whole screen, drawn by `render_tail` — the
shell's grid at the shell's own size and the `tail …` status line. Frames
keep arriving while it is open; when the stream ends (the shell exited, the
daemon went away) the model is marked exited and the view stays until
closed. Keys mean what they mean in `/tail` (`tail_key_action`): scroll,
`f` follow, `/` search with `n`/`N`, and `q` / `Esc` / `Ctrl-O` close the
view — **back to the panels exactly as they were**: focus, selections and
offsets. `Ctrl-C` quits `top` in every mode, as today. While the view is
open the panels keep refreshing underneath.

**Read-only.** The view sends nothing but the one `ShellTail` request; no
key reaches the shell as input, a signal or a resize.

## Constraints

- **The async shape is fixed — the same one `run_tui` uses twice already**
  (the crossterm reader, and the disk refresh after phase 04a): opening
  spawns a task that reads the connection's lines and sends each parsed
  `Response` on an `mpsc` channel, and the `select!` gets one arm that
  receives from it while a view is open. The channel closing means the
  stream ended. Closing the view drops the receiver and aborts the task.
  **Do not** hold `Lines::next_line()`, a `JoinHandle` or any other
  borrowed future across `select!` iterations, and do not read tokio's
  source to decide whether you may — a previous dispatch spent 30+ turns
  there.
- **Reuse, don't copy:** draw with `render_tail`; decide keys with
  `tail_key_action` over a translation of crossterm's `KeyEvent` to
  `cli::input::Key`, then `TailModel::act`. No second key table, no second
  grid painter.
- **A child module:** `top_tui.rs` is past the 800-line production cap
  (allowlisted at 848 after phase 04a; the PE accepted it as a ratchet
  entry, so raise that line by what this phase adds there and say why), so
  the view's state, its key translation and its forwarding task live in a
  new child module of `top_tui.rs` (declared as `top_tui_draw.rs` is), with
  its own inline tests. The forwarding task is generic over
  `AsyncBufRead`, so a test drives it with `tokio::io::duplex`.
- **Docs follow:** README's `daemoneye top` paragraph and its live-view
  table row describe `Enter`/`a`, the view and its keys; CLAUDE.md's
  key-files table gets a row for the new module.
- The allowlist is a ratchet; use `patch` with whole blocks on long files,
  never `patch_lines`.

## Spec

1. **Red tests first** — write these against the current code, stubbing
   new functions where needed, run them, keep the failing output for step 5
   (the `top_attach_` prefix matches no test today):
   - `top_attach_enter_or_a_opens_only_on_a_selected_shell` — `Enter` and
     `a` with Shells focused name the selected id; Runs focused, no rows,
     or editing the filter name nothing.
   - `top_attach_keys_close_on_q_esc_ctrl_o_and_ctrl_c_quits_top` — the
     translated keys give `tail_key_action`'s actions, search typing
     included; `Ctrl-C` sets `quit`.
   - `top_attach_view_draws_the_shell_and_close_restores_the_panels` — on a
     `TestBackend`: the grid's text and the `tail <id>` status line while
     open, no panel titles; after `Esc` the four panels with the same
     focus, selection and offsets.
   - `top_attach_stream_end_marks_exited_and_keeps_the_view` — a duplex
     feeds the forwarding task: a frame updates the grid; closing the writer
     closes the channel, the model is marked exited, the view stays open.
   - `top_attach_open_error_leaves_the_panels_and_names_it` — an error first
     frame opens nothing and the footer shows the message; the next key
     clears it.
2. **Opening** — the trigger, the footer hint and the error note on
   `TopModel`.
3. **The view** — the child module and the `run_tui` arm; drawing in
   `draw`.
4. **Docs** — per the constraint. `cargo test` passes with `failed=0`.
5. **Red runs** — one `### Update — <date> (red runs)` entry pasting step
   1's `test result:` lines and assertion messages verbatim from the run's
   output (no elisions or notes inside the fence).
6. **Findings** — one `### Findings — <date>` entry, two or three lines, or
   `none`.
7. **End-to-end** — run § End-to-end verification once before committing
   and paste its output, fenced, in an `### Update — <date> (end-to-end
   verification)` entry.
8. **Commit** as `feat: top attaches read-only to the selected shell (M36 phase-04b)`.

## Acceptance criteria

- [ ] The E2E block's first line is `top_attach passed=5 failed=0` (today
      `top_attach passed=0 failed=0`).
- [ ] The E2E block's `readonly_hits=0` line holds over every
      `src/cli/commands/top*.rs` file (preservation: today 0, and the new
      module must keep it).
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] The existing `top_tui` and `tail_loop_*` tests pass (preservation),
      notably `enter_opens_the_selected_runs_detail` and
      `esc_restores_focus_selection_and_offsets`.
- [ ] README's `daemoneye top` paragraph names `Enter`/`a` on a shell and
      the read-only view; CLAUDE.md names the new module.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0`, and `failed=0`.

## Test plan

Pure tests pin the trigger and the key translation; a duplex-driven test
pins the stream; `TestBackend` pins what the view draws and what closing
restores. The milestone's live check opens a shell from `top` on pinky,
watches it update, and returns with `q`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test top_attach_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "top_attach passed=" p, "failed=" f}'
cat src/cli/commands/top*.rs | grep -cE 'ShellInput|ShellSignal|ShellResize' | sed 's/^/readonly_hits=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/top_tui.rs`, `top_tui_draw.rs`, `src/cli/tail.rs`
  and `src/cli/tail_view.rs` (only to expose what the view reuses — no
  change to `/tail`'s behaviour), `README.md`, `CLAUDE.md`, and
  `tests/hygiene_allowlist.txt` (ratchet only).
- Create one child module of `top_tui.rs` for the attach view.

## Out of scope

- Typing into the shell from `top` (that is `/attach` in chat), pausing or
  signalling it, resizing it to the terminal.
- Opening a shell's recording (`/log`) from `top`.
- The Shells selection and P5 (phase 04a); `top --once`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-03 (escalation)

**Chosen lever:** refined re-dispatch, split into 04a + 04b.
**Rationale:** the first dispatch (as phase 04, run `683fa49e`) was
stopped by the PE after 514 turns with no source edit: 487 reads, 7
compactions, 22 parse failures. Four times it wrote the same design as
prose with no tool call, then the design was compacted away and it went
back to reading, including 31 greps of tokio's source on holding borrowed
futures across `select!`. The spec had left that async plumbing open, and
splitting off the selection + P5 halves the reading. The partial diff was
two status flips, so resuming would have preserved nothing.

### Notes for executor — 2026-11-03

The design is decided in § Constraints; the one question that stalled the
last run — how a frame stream lives inside `run_tui`'s `select!` — is
answered by the channel pattern `run_tui` already uses for crossterm
events. Read the referenced files once, write the red tests, then the code.
Each Spec step ends in a file write or a command run; do not write a design
document as a reply.
