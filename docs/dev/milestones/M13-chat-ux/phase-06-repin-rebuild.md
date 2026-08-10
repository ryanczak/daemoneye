# Phase 06: Deterministic bottom repin — rebuild the viewport, don't resize it

**Milestone:** M13 — Chat UX Polish
**Status:** in-progress
**Depends on:** phase-05
**Estimated diff:** ~130 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Phase-05 made resize/focus signals reach the renderer mid-stream, but the
repin they trigger — `reanchor()`, a same-size `Terminal::resize` inherited
from pre-M13 code — cannot actually fix a tmux window switch: live-checked
2026-08-10, artifacts persist and the input dialog can re-pin high on the
screen. This phase replaces `reanchor()`'s body with a deterministic bottom
repin: clear from the old viewport top downward, park the real cursor at the
new viewport top, and rebuild the `Terminal`.

## Architecture references

Read before starting:

- `docs/dev/NEXT.md` § "OPEN FINDING (2026-08-10)" — the full diagnosis this
  phase implements.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(Verified 2026-08-10 against the post-phase-05 tree.)

- `reanchor` (`src/cli/render_ratatui.rs:454-459`) lives in the **generic**
  `impl<B: Backend>` block:

  ```rust
  pub fn reanchor(&mut self) {
      if let Ok(size) = self.terminal.size() {
          let area = Rect::new(0, 0, size.width, size.height);
          let _ = self.terminal.resize(area);
      }
  }
  ```

- **Why `Terminal::resize` cannot repin** (derived from ratatui-core-0.1.2
  source, quoted so it is not re-litigated): `terminal/resize.rs:23` —
  resize full-clears the screen **only** `if next_area.width <
  self.viewport_area.width` (horizontal shrink); otherwise stale rows outside
  the recomputed viewport survive. The new origin comes from
  `compute_inline_size` (`terminal/inline.rs:390`), which anchors at
  `backend.get_cursor_position()` — a live DSR query returning wherever
  tmux's rewrap left the cursor — minus a stale internal offset. Nothing pins
  the viewport to the bottom.
- **The init path is deterministic where resize is not**: a fresh
  `Terminal::with_options(.., Viewport::Inline(h))` calls
  `compute_inline_size(&mut backend, height, area, 0)`
  (`terminal/init.rs:130` — offset **0**), so the viewport top lands exactly
  on the current cursor row, provided the cursor row leaves `height − 1`
  rows below it. That is the mechanism this phase exploits.
- **The scroll trap (pin this — it is the arithmetic an implementer gets
  wrong):** `compute_inline_size` runs
  `backend.append_lines(height − 1 − offset)` — with offset 0 and
  `VIEWPORT_ROWS = 6`, it appends **5** lines below the cursor. Parked at the
  bottom row, that scrolls the whole screen (and the visible history) up 5
  rows on *every* repin. Parked at row `size.height − VIEWPORT_ROWS`, the 5
  appended lines exactly fill the space below (`available_lines =
  height − row − 1 = 5`, `missing_lines = 0`) and **nothing scrolls**. The
  cursor park row is therefore `height − VIEWPORT_ROWS`, not `height − 1`.
- `VIEWPORT_ROWS: u16 = 6` is a private const in the same file (`:119`).
- The struct (`:162-166`) has fields `terminal`, `start_time`, `palette`.
  The stdout-specific impl block is `:172-197`
  (`impl RatatuiRenderer<CrosstermBackend<Stdout>>`, holding `new()`).
  `new()` enables raw mode and DEC modes (bracketed paste, focus events) —
  **terminal modes, not Terminal-object state**; a rebuild must NOT re-run
  them.
- The old viewport's top row is reachable as
  `self.terminal.get_frame().area().y` — `get_frame(&mut self)` is public
  (`ratatui-core-0.1.2/src/terminal/buffers.rs:51`) and `frame.area()` is the
  viewport area.
- All three `reanchor()` call sites are on the **concrete**
  `RatatuiRendererStdout` type (verified): `chat.rs:628` (SIGWINCH arm,
  followed by a `renderer.draw(...)`), `chat.rs:703` (`Key::FocusGained` arm,
  currently reanchor-only), `stream.rs:231` (`StreamOutcome::Reanchor` arm,
  redrawn by the next 80 ms spinner tick). No test calls `reanchor`.
- **DSR safety at the call sites:** the rebuild's init performs one
  `get_cursor_position` DSR round-trip on the tty. `AsyncStdin`
  (`input/tty.rs:31`) is a non-blocking `AsyncFd` read with **no background
  thread** — it consumes bytes only while a `read_key` future is being
  polled, and at all three call sites the enclosing `select!` has already
  returned, so no reader competes for the DSR reply. (Keystrokes the user
  types inside the ~ms DSR window are discarded by crossterm's reply filter —
  accepted, do not try to fix.)
- Grep baselines: `fn repin_rows` 0; `FromCursorDown` 0;
  `self.terminal.resize` 1 (the body being replaced).

## Spec

### Task 1 — Pure row math: `repin_rows`

In `src/cli/render_ratatui.rs`, add a free function near `split_spinner_row`
— pinned exactly (its subtraction is mutation M1's target):

```rust
/// Rows for a bottom repin: (clear_from, cursor_park).
///
/// `cursor_park` is the row the real cursor must sit on when the Terminal
/// is rebuilt — the future viewport TOP (`height − VIEWPORT_ROWS`), never
/// the bottom row: ratatui's inline init appends `VIEWPORT_ROWS − 1` lines
/// below the cursor, which scrolls the screen when parked lower (see the
/// phase-06 doc's scroll-trap note). `clear_from` wipes from the old
/// viewport top or the new one, whichever is higher on screen.
fn repin_rows(old_top: u16, height: u16) -> (u16, u16) {
    let park = height.saturating_sub(VIEWPORT_ROWS);
    (old_top.min(park), park)
}
```

### Task 2 — Move `reanchor` to the stdout impl and rebuild

1. **Delete** `reanchor` from the generic `impl<B: Backend>` block
   (`:454-459`).
2. Add it to the stdout-specific impl block (`:172`, alongside `new()`) with
   the rebuild body:

   ```rust
   /// Deterministically re-pin the inline viewport to the bottom of the
   /// terminal after tmux moved or rewrapped the screen (window switch,
   /// resize). `Terminal::resize` cannot do this — it anchors relative to
   /// the live cursor and only clears on horizontal shrink — so instead:
   /// wipe from the old viewport top downward, park the cursor exactly at
   /// the new viewport top, and rebuild the Terminal (inline init anchors
   /// at the cursor row with offset 0).
   pub fn reanchor(&mut self) {
       use crossterm::cursor::MoveTo;
       use crossterm::execute;
       use crossterm::terminal::{Clear, ClearType};

       let Ok(size) = self.terminal.size() else {
           return;
       };
       let old_top = self.terminal.get_frame().area().y;
       let (clear_from, park) = repin_rows(old_top, size.height);
       let mut out = std::io::stdout();
       if execute!(
           out,
           MoveTo(0, clear_from),
           Clear(ClearType::FromCursorDown),
           MoveTo(0, park)
       )
       .is_err()
       {
           return;
       }
       let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
       if let Ok(terminal) = Terminal::with_options(
           backend,
           ratatui::TerminalOptions {
               viewport: ratatui::Viewport::Inline(VIEWPORT_ROWS),
           },
       ) {
           self.terminal = terminal;
       }
   }
   ```

   Do **not** call `Self::new()` or re-run `enable_raw_mode` /
   `EnableBracketedPaste` / `EnableFocusChange` — those are terminal modes
   already in effect; only the `Terminal` object is replaced. On any error
   the old terminal stays — degraded but never torn down.

All three call sites are on `RatatuiRendererStdout`, so they compile
unchanged. No `TestBackend` path needs `reanchor` (no test calls it).

### Task 3 — Redraw after repin at the FocusGained site

In `src/cli/commands/chat.rs`'s `Key::FocusGained` arm (`:700-705`), the
rebuilt viewport is blank until the next draw; the SIGWINCH arm already
draws but this arm does not. After `renderer.reanchor();`, add the same
redraw the SIGWINCH arm performs (build the `StatusBarState` from the
variables in scope there and call `renderer.draw(state.current_line(), &sb)`)
— mirror the sibling arm at `:620-631` exactly. The `stream.rs` Reanchor arm
needs no change (the 80 ms tick redraws).

### Task 4 — Tests

Write the tests named in § Test plan (pure `repin_rows` cases — the rebuild
itself is live-only behavior, covered by the milestone-close live check).

### Task 5 — Mutation M1 apply + restore (park row)

Apply a `patch` on `src/cli/render_ratatui.rs` changing
`let park = height.saturating_sub(VIEWPORT_ROWS);` to
`let park = height.saturating_sub(1);`, then:

```sh
echo "== M1 APPLIED ==" >> /tmp/e2e-m13-06.txt
cargo test --lib repin_rows 2>&1 | tail -5 >> /tmp/e2e-m13-06.txt
```

`repin_rows_parks_at_viewport_top` must show **FAILED**. If it stays green,
report a blocker — do not adjust a test to make it fail. Restore with the
inverse `patch`, then:

```sh
echo "== M1 RESTORED ==" >> /tmp/e2e-m13-06.txt
grep -c 'height.saturating_sub(1)' src/cli/render_ratatui.rs >> /tmp/e2e-m13-06.txt
cargo test --lib repin_rows 2>&1 | tail -5 >> /tmp/e2e-m13-06.txt
```

The grep count must be `0` and the tests green.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
append a new Update Log entry headed
`### Update — <date> (end-to-end verification)` whose fenced block is the
contents of `/tmp/e2e-m13-06.txt`, **inserted by command (`cat >>`), never
retyped**. Then run this self-check and paste its output as the entry's last
line, outside the fence:

```sh
awk '/^### Update — .*\(end-to-end verification\)/{f=1} f' docs/dev/milestones/M13-chat-ux/phase-06-repin-rebuild.md | sed -n '/^```$/,/^```$/p' | sed '1d;$d' > /tmp/pasted-06.txt
diff /tmp/pasted-06.txt /tmp/e2e-m13-06.txt > /dev/null && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The run is finished only when this prints `PASTE MATCH`. The server-authored
`(complete)` entry does not satisfy Task 6.

## Acceptance criteria

Progress markers — each **fails against the current tree** (verified at
drafting):

- [ ] `grep -c 'fn repin_rows' src/cli/render_ratatui.rs` prints `1`.
      (Currently: 0.)
- [ ] `grep -c 'FromCursorDown' src/cli/render_ratatui.rs` prints `1`.
      (Currently: 0.)
- [ ] `grep -c 'self.terminal.resize' src/cli/render_ratatui.rs` prints `0`
      — the resize-based repin is gone. (Currently: 1.)
- [ ] Tests `repin_rows_parks_at_viewport_top`,
      `repin_rows_clears_from_old_top_when_higher`,
      `repin_rows_short_terminal_saturates` pass. (Currently: none exist.)

No-regression guards — these **already pass** and must still pass (they are
not evidence of new work):

- [ ] The phase-05 suites still pass: `focus_outcome_maps_focus_gained_to_reanchor`,
      `select_stream_focus_gained_returns_reanchor`,
      `select_stream_sigwinch_returns_reanchor`.
- [ ] `input_box_row_is_stable_across_draw_modes` and the phase-04 cursor
      tests still pass (the generic impl loses only `reanchor`; nothing else
      moves).
- [ ] Four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

In `src/cli/render_ratatui.rs` `mod tests`:

- `repin_rows_parks_at_viewport_top` — `repin_rows(10, 24)` == `(10, 18)`:
  park is `24 − 6`, never `23`. (Mutation M1 target.)
- `repin_rows_clears_from_old_top_when_higher` — `repin_rows(3, 24)` ==
  `(3, 18)` (old viewport higher → clear from it), and `repin_rows(20, 24)`
  == `(18, 18)` (old viewport below the new top → clear from the new top).
- `repin_rows_short_terminal_saturates` — `repin_rows(0, 4)` == `(0, 0)`
  (terminal shorter than the viewport: park saturates to row 0).

## End-to-end verification

```sh
: > /tmp/e2e-m13-06.txt
echo "== GATES ==" >> /tmp/e2e-m13-06.txt
cargo fmt --all 2>&1 | tail -2 >> /tmp/e2e-m13-06.txt; echo "fmt exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-06.txt
cargo build 2>&1 | tail -2 >> /tmp/e2e-m13-06.txt; echo "build exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-06.txt
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2 >> /tmp/e2e-m13-06.txt; echo "clippy exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-06.txt
cargo test 2>&1 | grep -E '^test result' >> /tmp/e2e-m13-06.txt; echo "test exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-06.txt
echo "== SURFACES ==" >> /tmp/e2e-m13-06.txt
echo "repin_rows fns: $(grep -c 'fn repin_rows' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-06.txt
echo "clear-down calls: $(grep -c 'FromCursorDown' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-06.txt
echo "resize repins left: $(grep -c 'self.terminal.resize' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-06.txt
wc -l /tmp/e2e-m13-06.txt >> /tmp/e2e-m13-06.txt
```

(The Task 5 mutation runs append into the same file in task order.)

**The decisive verification is live and human:** rebuild + reinstall, restart
`daemoneye chat`, switch tmux windows during and between streamed turns —
same-width and different-width layouts — and confirm the input dialog re-pins
to the bottom with history above it. That check happens at the milestone
gate, not in this phase's executor block.

## Authorizations

None.

## Out of scope

- Re-rendering scrollback tmux rewrapped above the viewport — immutable by
  design (milestone non-goal); a width-change switch may leave history
  mangled until new content scrolls in.
- The DSR keystroke-discard window during rebuild — accepted.
- `restore()`, `new()`, raw-mode or DEC-mode handling — unchanged.
- Anything in `stream.rs`, `editor.rs`, `src/daemon/`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-10 02:44 (started)

**Executor:** Claude (Sonnet 4.5)
**Working on:** Tasks 1–6 — repin_rows, rebuild reanchor, FocusGained redraw, tests, mutation M1, e2e

### Update — 2026-08-10 02:47 (end-to-end verification)

```
== M1 APPLIED ==
    cli::render_ratatui::tests::repin_rows_short_terminal_saturates

test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 1228 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
== M1 RESTORED ==
0
test cli::render_ratatui::tests::repin_rows_short_terminal_saturates ... ok
test cli::render_ratatui::tests::repin_rows_parks_at_viewport_top ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 1228 filtered out; finished in 0.00s

== GATES ==
fmt exit=0
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.01s
build exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.23s
clippy exit=0
test result: ok. 1231 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit=0
== SURFACES ==
repin_rows fns: 4
clear-down calls: 1
resize repins left: 0
33 /tmp/e2e-m13-06.txt
```
