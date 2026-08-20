# Phase 07: Viewer Mouse

**Milestone:** M17 — Transcript View
**Status:** in-progress
**Depends on:** phase-04 (search — `key_action`/`ViewerAction` are the decode
surface this extends) and phase-03 (collapse — click-to-toggle acts on it).
**Estimated diff:** ~420 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Wheel-scroll the transcript and click a block header to expand or collapse it —
**inside the alternate screen only**. Mouse reporting is switched on when the
viewer opens and off from the same `Drop` that leaves the alternate screen, so
no error path can strand the chat surface with mouse tracking enabled.

## Architecture references

Read before starting:

- `docs/design/transcript-view.md` — §"Screen handling" (the viewer owns every
  scroll path inside the alt screen) and §"Non-goal" (why the inline surface
  never enables mouse).
- `src/cli/viewer.rs` — 1400+ lines; read `run_transcript_viewer`,
  `AltScreenGuard`, `key_action` and `render_transcript` before editing.
- `src/cli/input/tty.rs` — the hand-rolled escape parser this extends.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The viewer's entry and its guard** (`src/cli/viewer.rs`):

```rust
    execute!(std::io::stdout(), EnterAlternateScreen)?;

    // From here on the screen is owned by this guard: leaving it and re-pinning
    // the inline viewport runs from `Drop`, so `?`, `break` and `Ok(())` all
    // exit the same way. `let _ =` is required — a `Drop` cannot propagate.
    let _guard = AltScreenGuard::new(|| {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), Show);
        renderer.reanchor();
    });
```

`AltScreenGuard` has **no `armed` field and no `disarm`** — its `Drop` is
unconditional. That property was bought with two bounces; this phase extends
the teardown closure and must not weaken it.

**The key parser** is `read_key` (`src/cli/input/tty.rs`), a hand-rolled
byte-at-a-time reader with a 30 ms inter-byte timeout. The CSI arms look like
this (`tty.rs:212-216`):

```rust
                        Ok(Some(b'6')) => {
                            // \x1b[6~ = PageDown
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::PageDown
                        }
```

There is **no `<` arm** today, so an SGR mouse report currently falls to the
CSI catch-all and its digits leak out as stray `Key::Char`s.

**The decode surface** is `key_action(key, searching) -> ViewerAction`
(phase-04), with 20 variants, and `viewer_loop` matches every one of them.

**The layout** in `render_transcript` (`viewer.rs:354`) is
`body_height = area.height.saturating_sub(1)` — rows occupy
`area.y .. area.y + body_height`, and the status line is the last row.

### Four gotchas, each verified against the tree

1. **Mouse must never be enabled on the inline surface.** Enabling tracking
   makes the terminal send reports instead of doing its own drag-select, so the
   user loses selection in the *chat* pane. Enable on viewer entry, disable in
   the guard's teardown — nowhere else. A criterion greps `src/cli/` outside
   `viewer.rs` for the enable sequence and expects zero hits.
2. **Disable belongs in the `Drop`, not after the loop.** If it sits after
   `viewer_loop(...).await?`, an error return skips it and the chat session runs
   with mouse reporting on — every mouse move spraying escape sequences into the
   prompt. This is exactly the phase-02 failure re-run on a new resource; the
   guard already exists, so extend its closure.
3. **SGR is `ESC [ < Cb ; Cx ; Cy M|m`** — parameters are *decimal digits*, not
   single bytes, so the parser must accumulate until the terminator. `M` is
   press, `m` is release. Wheel-up is `Cb == 64`, wheel-down is `Cb == 65`.
   Coordinates are **1-based**. Do not assume single-digit fields: a click at
   column 137 sends three digits.
4. **The 30 ms inter-byte timeout applies to every byte** of the sequence. Read
   the digits in the same timeout-guarded loop the existing arms use; a partial
   sequence must degrade to a harmless key rather than hanging or emitting
   garbage.

## Spec

### Task 1 — Parse SGR mouse reports

In `src/cli/input/tty.rs`:

- Add to `Key`:

  ```rust
      /// A mouse report from inside the transcript viewer (SGR 1006).
      /// `col`/`row` are 0-based, converted from the wire's 1-based values.
      Mouse { button: u8, col: u16, row: u16, pressed: bool },
  ```

- Add a `<` arm to the CSI match that reads `Cb`, `Cx`, `Cy` as decimal fields
  separated by `;`, terminated by `M` (pressed = true) or `m` (pressed = false),
  each byte read through the same `timeout(Duration::from_millis(30), …)` guard
  the neighbouring arms use. Convert coordinates to 0-based by subtracting 1
  (saturating).
- A malformed or truncated sequence yields `Key::Char('\0')` — the parser's
  existing "ignore" value. It must **not** hang and must **not** emit the raw
  digits as `Key::Char`s.

### Task 2 — Enable and disable, both bound to the guard

In `src/cli/viewer.rs`, in `run_transcript_viewer`:

- Immediately after `EnterAlternateScreen`, enable SGR mouse reporting:
  `execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?`.
- Add the matching disable to the **existing guard closure**, before
  `LeaveAlternateScreen`:
  `let _ = execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);`

Do not add a second guard, do not reintroduce `disarm`, and do not disable
after the loop instead of in the `Drop`.

### Task 3 — Hit-testing, as a pure function

In `src/cli/viewer.rs`:

```rust
/// Which transcript row a mouse row lands on, or `None` for the status line
/// or out of range. `area_y` is the body's first screen row, `body_height`
/// its height, `scroll` the current offset, `total` the row count.
pub fn row_at(
    mouse_row: u16,
    area_y: u16,
    body_height: u16,
    scroll: usize,
    total: usize,
) -> Option<usize>
```

Pinned: a `mouse_row` below `area_y`, or at/after `area_y + body_height` (the
status line), or resolving to an index `>= total`, yields `None`. Otherwise
`Some(scroll + (mouse_row - area_y) as usize)`.

### Task 4 — Decode mouse into actions

Extend `key_action` (it stays pure, and stays the only decode point):

- `Key::Mouse { button: 64, pressed: true, .. }` → `ViewerAction::ScrollUp`
- `Key::Mouse { button: 65, pressed: true, .. }` → `ViewerAction::ScrollDown`
- `Key::Mouse { button: 0, pressed: true, col, row }` → a new
  `ViewerAction::ClickAt { col, row }`
- Every **release** (`pressed: false`) → `ViewerAction::Ignore`
- Any other button → `ViewerAction::Ignore`

While `searching` is true, wheel scrolling still scrolls (matching the existing
`(_, Key::Up)` precedent) but `ClickAt` decodes to `Ignore` — a stray click must
not silently reshape the transcript mid-query.

### Task 5 — Act on a click

In `viewer_loop`, handle `ViewerAction::ClickAt { row, .. }`:

1. `row_at(...)` → `None` means do nothing at all.
2. Otherwise take the clicked `ViewRow`'s `block` index, set `focus` to it, and
   **if the clicked row is that block's header row** (`RowKind::Header`), toggle
   its membership in `collapsed` — the same toggle `Enter` performs. A click on
   a body row focuses without collapsing.
3. Recompute matches after a toggle, exactly as the `ToggleCollapse` arm does,
   so match indices never point past the end of `rows`.

Scroll wheel needs no new handling — it decodes to the existing scroll actions.

### Task 6 — Tests

Write the tests named in § Test plan. All are pure: `row_at` and `key_action`
need no terminal, and the parser tests use the existing pipe-backed `AsyncStdin`
idiom already used by the `read_key_*` tests in `tty.rs`.

### Task 7 — Mutation M1: apply

Use the `patch` tool on `src/cli/viewer.rs`.

- `old_str`: `    if mouse_row >= area_y + body_height {`
- `new_str`: `    if false {`

(the status-line guard inside `row_at`; if your guard is written with a
different but equivalent condition, mutate **that** line to `if false {`.)

Then run, appending to the evidence artifact:

```sh
A=/tmp/e2e-07.txt
echo "== M1 APPLIED ==" >> "$A"
grep -c 'if false {' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

The run **must fail** — `row_at_rejects_the_status_line` is what proves the
guard is real. A green run means the test is vacuous; stop and file a blocker.

### Task 8 — Mutation M1: restore

`patch` the line back, then:

```sh
A=/tmp/e2e-07.txt
echo "== M1 RESTORED ==" >> "$A"
grep -c 'if false {' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

`grep -c` must print `1` after task 7 and `0` after task 8. Do **not** use
`git checkout` to restore — the file holds this round's uncommitted work.

### Task 9 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-07.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

### Task 10 — PASTE MATCH self-check

After pasting, run:

```sh
D=docs/dev/milestones/M17-transcript-view/phase-07-viewer-mouse.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-07.txt
diff /tmp/pasted-07.txt /tmp/e2e-07.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the literal verdict line into that same Update Log entry, below the
fence.

## Acceptance criteria

Every criterion asserts an observed value or count.

- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` passes.
- [ ] Test `read_key_parses_sgr_wheel_up` passes — feeding `\x1b[<64;10;5M`
      yields `Key::Mouse { button: 64, col: 9, row: 4, pressed: true }`
      (0-based conversion asserted on the exact values).
- [ ] Test `read_key_parses_multi_digit_mouse_coords` passes — feeding
      `\x1b[<0;137;42M` yields `col: 136, row: 41`. This is the negative case
      for single-digit parsing.
- [ ] Test `read_key_mouse_release_is_not_pressed` passes — a `…m` terminator
      yields `pressed: false`.
- [ ] Test `row_at_rejects_the_status_line` passes — with `area_y = 0`,
      `body_height = 10`, a `mouse_row` of 10 yields **`None`**, while 9 yields
      `Some(scroll + 9)`; and a row resolving past `total` yields `None`.
- [ ] Test `key_action_wheel_scrolls_click_ignored_while_searching` passes —
      with `searching = true`, button 64 → `ScrollUp` **and** button 0 →
      `Ignore`; with `searching = false`, button 0 → `ClickAt { .. }`.
- [ ] Test `key_action_mouse_release_is_ignored` passes — `pressed: false`
      yields `Ignore` for buttons 0, 64 and 65.
- [ ] `grep -c "EnableMouseCapture" src/cli/viewer.rs` prints at least 1, and
      `grep -rn "EnableMouseCapture" src/cli/ --include=*.rs | grep -v "viewer.rs" | wc -l`
      prints `0` — mouse is enabled in the viewer and nowhere else.
- [ ] `awk '/impl Drop for/{f=1} f&&/DisableMouseCapture/{print "GUARD OK"; exit}' src/cli/viewer.rs`
      prints `GUARD OK`, **or** the disable lives in the closure passed to
      `AltScreenGuard::new` — in which case
      `awk '/AltScreenGuard::new/{f=1} f&&/DisableMouseCapture/{print "GUARD OK"; exit}' src/cli/viewer.rs`
      prints `GUARD OK`. One of the two must print it; the disable must not sit
      after the loop.
- [ ] `grep -c "disarm" src/cli/viewer.rs` prints `0` — the phase-02 contract is
      intact and the guard is still unconditional.
- [ ] `/tmp/e2e-07.txt` shows `== M1 APPLIED ==` with a **failing** run and
      `grep -c` = 1, then `== M1 RESTORED ==` with a passing run and
      `grep -c` = 0.
- [ ] The Update Log's newest entry is headed
      `### Update — <date> (end-to-end verification)`, contains the pasted
      artifact, and ends with the literal line `PASTE MATCH`.

## Test plan

In `src/cli/input/tty.rs` (`#[cfg(test)] mod tests`). Use the helpers the
existing `read_key_*` tests already use, by name — they are in that module
today: `make_pipe_stdin()` (`tty.rs:353`), `write_bytes(...)`, and
`read_key_bounded(...)` (`tty.rs:410`) / `read_key_within(...)` (`tty.rs:415`)
for the bounded reads:

- `read_key_parses_sgr_wheel_up` — exact struct equality.
- `read_key_parses_multi_digit_mouse_coords` — `col: 136, row: 41`.
- `read_key_mouse_release_is_not_pressed` — `pressed: false`.
- `read_key_malformed_mouse_sequence_is_ignored` — a truncated
  `\x1b[<64;10` followed by nothing yields `Key::Char('\0')` via
  `read_key_within(...)` with a short bound, and does not hang. Note
  `read_key_within_panics_when_no_byte_ever_arrives` (`tty.rs:543`) is the
  existing precedent for asserting the bounded-read behaviour.

In `src/cli/viewer.rs`:

- `row_at_rejects_the_status_line` — the three cases in the criteria.
- `row_at_maps_body_rows_with_scroll` — `area_y = 2`, `scroll = 30`,
  `mouse_row = 5` → `Some(33)`.
- `key_action_wheel_scrolls_click_ignored_while_searching` — both modes.
- `key_action_mouse_release_is_ignored` — three buttons.

## End-to-end verification

Mouse behaviour is only observable with a terminal and a hand on the wheel, so
the real check is live and architect-run at milestone close. What the executor
verifies here is the parser, the hit-test arithmetic, the decode table, and the
structural guarantees that keep mouse tracking inside the viewer.

Tasks 7 and 8 append the mutation pair to the **same** artifact before this
block runs; do not truncate `/tmp/e2e-07.txt` here.

```sh
A=/tmp/e2e-07.txt
echo "== GATES ==" >> "$A"
cargo fmt --all -- --check 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "fmt exit=${PIPESTATUS[0]}" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "clippy exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "test exit=${PIPESTATUS[0]}" >> "$A"
echo "== VIEWER UNITS ==" >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -30 >> "$A"
echo "units exit=${PIPESTATUS[0]}" >> "$A"
echo "== TTY UNITS ==" >> "$A"
cargo test --lib cli::input 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -30 >> "$A"
echo "tty exit=${PIPESTATUS[0]}" >> "$A"
echo "== MOUSE ENABLED ONLY IN THE VIEWER ==" >> "$A"
grep -c "EnableMouseCapture" src/cli/viewer.rs >> "$A"
grep -rn "EnableMouseCapture" src/cli/ --include=*.rs | grep -v "viewer.rs" | wc -l >> "$A"
echo "== DISABLE IS BOUND TO THE GUARD ==" >> "$A"
awk '/AltScreenGuard::new/{f=1} f&&/DisableMouseCapture/{print "GUARD OK"; exit}' src/cli/viewer.rs >> "$A"
awk '/impl Drop for/{f=1} f&&/DisableMouseCapture/{print "GUARD OK (drop)"; exit}' src/cli/viewer.rs >> "$A"
echo "== PHASE-02 CONTRACT STILL HOLDS ==" >> "$A"
grep -c "disarm" src/cli/viewer.rs >> "$A"
grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs >> "$A"
echo "teardown grep exit=$?  (1 = none found, which is the pass)" >> "$A"
```

## Authorizations

- [ ] May edit `src/cli/viewer.rs` and `src/cli/input/tty.rs`.

No new dependencies — `crossterm` 0.29 already provides
`EnableMouseCapture` / `DisableMouseCapture`. `docs/architecture.md` is **not**
authorized.

## Out of scope

- **Mouse anywhere but the viewer.** The inline chat surface never enables
  tracking — a criterion enforces it.
- **Drag selection, double-click, right-click menus, motion events.** Wheel and
  single left-click only; every other report decodes to `Ignore`.
- **X10/urxvt mouse encodings.** SGR (1006) only; `crossterm`'s
  `EnableMouseCapture` requests it.
- **Click-to-copy or click-to-search.** Phase-05's `y` and phase-04's `/` stay
  keyboard-only.
- **Undoing anything phases 02–06 established.** `AltScreenGuard` keeps its
  unconditional `Drop` and gains no `disarm`; `viewer.rs` gains no
  `try_restore` / `disable_raw_mode` / `.restore()`; the client still never
  reads `var/log/panes/`. The E2E block re-checks these.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-20 15:25 (progress)

Started phase 07. Implemented the SGR mouse parser (`read_sgr_mouse` in
`src/cli/input/tty.rs`, with `Key::Mouse`), the enable/disable pair bound to the
`AltScreenGuard` closure, the `row_at` hit-test, the `ClickAt` decode and the
click handler in `viewer_loop`. All new tests pass and `row_at_rejects_the_status_line`
kills the M1 mutation. Proceeding to end-to-end capture.

### Update — 2026-08-20 15:33 (end-to-end verification)

```sh
== M1 APPLIED ==
1
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok

failures:

---- cli::viewer::tests::row_at_rejects_the_status_line stdout ----

thread 'cli::viewer::tests::row_at_rejects_the_status_line' (3278765) panicked at src/cli/viewer.rs:1494:9:
assertion `left == right` failed: status line must be rejected
  left: Some(10)
 right: None
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    cli::viewer::tests::row_at_rejects_the_status_line

test result: FAILED. 36 passed; 1 failed; 0 ignored; 0 measured; 1343 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
exit=101
== M1 RESTORED ==
0
exit=1
== GATES ==
fmt exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.24s
clippy exit=0

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s

     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)

running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

test exit=0
== VIEWER UNITS ==
test cli::viewer::tests::find_matches_is_case_insensitive ... ok
test cli::viewer::tests::expanded_layout_is_unchanged_by_the_new_path ... ok
test cli::viewer::tests::collapse_toggle_is_involutive ... ok
test cli::viewer::tests::focus_next_wraps_at_last_block ... ok
test cli::viewer::tests::focus_prev_wraps_at_first ... ok
test cli::viewer::tests::key_action_commands_apply_when_not_searching ... ok
test cli::viewer::tests::key_action_escape_cancels_search_but_quits_otherwise ... ok
test cli::viewer::tests::find_matches_skips_collapsed_block_bodies ... ok
test cli::viewer::tests::key_action_mouse_release_is_ignored ... ok
test cli::viewer::tests::key_action_typing_wins_over_commands_while_searching ... ok
test cli::viewer::tests::key_action_wheel_scrolls_click_ignored_while_searching ... ok
test cli::viewer::tests::key_action_y_copies_only_when_not_searching ... ok
test cli::viewer::tests::layout_blocks_empty_transcript_is_empty ... ok
test cli::viewer::tests::layout_blocks_separates_blocks_with_one_blank ... ok
test cli::viewer::tests::next_match_wraps ... ok
test cli::viewer::tests::prev_match_wraps ... ok
test cli::viewer::tests::layout_blocks_wraps_to_width ... ok
test cli::viewer::tests::row_at_maps_body_rows_with_scroll ... ok
test cli::viewer::tests::row_at_rejects_the_status_line ... ok
test cli::viewer::tests::rows_carry_their_source_block_index ... ok
test cli::viewer::tests::collapsed_output_lays_out_as_exactly_one_row ... ok
test cli::viewer::tests::scroll_to_row_only_moves_when_offscreen ... ok
test cli::viewer::tests::layout_blocks_renders_full_output ... ok
test cli::viewer::tests::render_transcript_draws_rows_into_backend ... ok
test cli::viewer::tests::render_transcript_shows_match_counter ... ok
test cli::viewer::tests::render_transcript_marks_collapsed_and_focused ... ok
test cli::viewer::tests::render_transcript_survives_scroll_past_end ... ok

test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 1343 filtered out; finished in 0.00s

units exit=0
== TTY UNITS ==
test cli::input::editor::tests::delete_newline_joins_lines ... ok
test cli::input::editor::tests::visual_lines_cursor_matches_wrap ... ok
test cli::input::editor::tests::visual_lines_cursor_on_wrapped_overlong ... ok
test cli::input::editor::tests::push_history_clears_current_line ... ok
test cli::input::editor::tests::visual_lines_empty ... ok
test cli::input::editor::tests::visual_lines_hard_newline ... ok
test cli::input::editor::tests::submit_preserves_newlines ... ok
test cli::input::editor::tests::visual_lines_leading_whitespace ... ok
test cli::input::editor::tests::visual_lines_preserves_whitespace ... ok
test cli::input::editor::tests::visual_lines_overlong_word ... ok
test cli::input::editor::tests::visual_lines_single_line_no_wrap ... ok
test cli::input::editor::tests::visual_lines_wraps_at_word_boundary ... ok
test cli::input::tty::tests::read_key_bracketed_paste_single_line ... ok
test cli::input::tty::tests::read_key_bare_cr_yields_enter ... ok
test cli::input::tty::tests::read_key_alt_enter_lf_yields_ctrlj ... ok
test cli::input::tty::tests::read_key_arrow_up ... ok
test cli::input::tty::tests::read_key_alt_enter_yields_ctrlj ... ok
test cli::input::tty::tests::read_key_backspace ... ok
test cli::input::tty::tests::read_key_bare_lf_yields_enter ... ok
test cli::input::tty::tests::read_key_bracketed_paste_no_enter_mid_paste ... ok
test cli::input::tty::tests::read_key_bracketed_paste_multiline ... ok
test cli::input::tty::tests::read_key_char ... ok
test cli::input::tty::tests::read_key_parses_sgr_wheel_up ... ok
test cli::input::tty::tests::read_key_parses_multi_digit_mouse_coords ... ok
test cli::input::tty::tests::read_key_mouse_release_is_not_pressed ... ok
test cli::input::tty::tests::read_key_malformed_mouse_sequence_is_ignored ... ok
test cli::input::tty::tests::read_key_within_panics_when_no_byte_ever_arrives - should panic ... ok

test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 1344 filtered out; finished in 0.05s

tty exit=0
== MOUSE ENABLED ONLY IN THE VIEWER ==
2
0
== DISABLE IS BOUND TO THE GUARD ==
GUARD OK
GUARD OK (drop)
== PHASE-02 CONTRACT STILL HOLDS ==
0
teardown grep exit=1  (1 = none found, which is the pass)
```
PASTE MATCH
