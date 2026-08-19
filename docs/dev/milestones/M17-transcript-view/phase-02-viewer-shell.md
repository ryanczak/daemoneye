# Phase 02: Viewer Shell

**Milestone:** M17 — Transcript View
**Status:** todo
**Depends on:** phase-01 (transcript-model, `done`)
**Estimated diff:** ~430 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Open the phase-01 transcript in an alternate-screen viewer with `ctrl+o`, scroll
it, and close back to an inline chat surface identical to the one the user left.
Read-only: every block renders in full — including the output the inline panel
elided — with no expand/collapse, search, copy or mouse (phases 03–07).

## Architecture references

Read before starting:

- `docs/design/transcript-view.md` — §"Screen handling" is the enter/exit
  contract this phase implements; §"Non-goal" is why the inline surface is not
  touched.
- `docs/dev/milestones/M17-transcript-view/README.md` § Notes — the two gaps
  carried out of phase-01's review, which tasks 7 and 8 close.
- `CLAUDE.md` § "Key files" — `src/cli/render_ratatui.rs` is the inline
  renderer; this phase adds a **second**, separate screen rather than changing
  it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The transcript exists and is populated** (phase-01, commit `a49ebca`).
`src/cli/transcript.rs` exposes `Block` (`UserTurn { label, text }`,
`Assistant { text }`, `ToolPanel { tool, summary, label }`,
`Output { tool_call_id, full, shown }`, `System { text }`) and `Transcript`
with `blocks() -> &[Block]`, `len()`, `is_empty()`, `evicted()`, `push()`,
`append_assistant()`, `with_caps()`.

`run_chat_ratatui` (`src/cli/commands/chat.rs:271`) owns it:

```rust
    let mut transcript = crate::cli::transcript::Transcript::new();
```

**The input loop** is `read_input_line_inner_ratatui`
(`src/cli/commands/chat.rs:614`), whose context struct is
(`chat.rs:597-612`):

```rust
struct RatatuiInputCtx<'a> {
    state: &'a mut InputState,
    stdin: &'a AsyncStdin,
    sigwinch: &'a mut tokio::signal::unix::Signal,
    renderer: &'a mut RatatuiRendererStdout,
    chat_width: &'a mut usize,
    session_id: &'a str,
    approval: &'a SessionApproval,
    model: &'a str,
    prompt_tokens: u32,
    context_window: u32,
    daemon_up: bool,
    cost_usd: f64,
    has_untracked: bool,
    last_ctrl_c: &'a mut Option<std::time::Instant>,
}
```

Its single call site is `chat.rs:397`. The loop body is a
`tokio::select!` over `sigwinch.recv()` and `read_key(stdin)`; the sigwinch arm
already does `*chat_width = terminal_width(); renderer.reanchor();` then
redraws — **that is the restore shape this phase reuses on viewer exit.**

**Three key-parsing facts that will cost a bounce if missed** — all in
`src/cli/input/tty.rs`, in `read_key` (line 161):

1. **There is no `Key::Esc`.** A bare Escape falls through to the catch-all
   `_ => Key::Char('\x1b')` (tty.rs:244). The viewer's exit key must match
   `Key::Char('\x1b')`.
2. **Ctrl+O is currently swallowed.** `c if c < 0x20 => Key::Char('\0')`
   (tty.rs:247) eats every unhandled control byte, so `0x0f` becomes
   `Char('\0')` today. A new `b'\x0f' => Key::CtrlO` arm must sit **with the
   other control-byte arms** (`b'\x0b' => Key::CtrlK` at tty.rs:172), before
   that catch-all.
3. **PageUp/PageDown are not parsed.** `ESC[5~` / `ESC[6~` hit the CSI
   catch-all, and the trailing `~` is then delivered as a separate
   `Key::Char('~')`. New arms must consume it, exactly like the Delete arm
   (tty.rs:187-191):

   ```rust
                        Ok(Some(b'3')) => {
                            // \x1b[3~ = Delete
                            let _ = timeout(Duration::from_millis(30), stdin.read_byte()).await;
                            Key::Delete
                        }
   ```

**The renderer's restore contract.** `RatatuiRenderer::restore()`
(`src/cli/render_ratatui.rs:851`) calls `ratatui::try_restore()`, which
**disables raw mode**. It is the end-of-session teardown. The viewer must
**never** call it — the chat session continues after the viewer closes and
needs raw mode intact. The viewer's exit path is `LeaveAlternateScreen` +
`renderer.reanchor()`.

`reanchor()` (`render_ratatui.rs:286`) is what re-pins the inline viewport
after the screen moved underneath it; it is already the sigwinch response, so
calling it after leaving the alternate screen is the same operation.

**Deps:** `ratatui = "0.30"` (features `crossterm`, `scrolling-regions`),
`crossterm = "0.29"`. `ratatui::backend::TestBackend` is the project's headless
render-test idiom; `Terminal::new(TestBackend::new(w, h))` gives a fullscreen
viewport (`render_ratatui.rs:1138` uses `Terminal::with_options` for the
*inline* case — the viewer wants the plain fullscreen `Terminal::new`).

**Two carried gaps from phase-01's review** (README § Notes), closed here:

- `Transcript::append_assistant` (`transcript.rs:101`) appends to an existing
  block without calling `evict()`, so the byte cap goes unenforced while one
  assistant turn streams.
- Two panels reach scrollback without reaching the transcript: the
  `ToolFinished` arm's `None` branch (`src/cli/commands/stream.rs:678`) and the
  end-of-turn flush of a started-but-never-finished tool (`stream.rs:729-732`):

  ```rust
    // Flush a started-but-never-finished tool so its panel is not lost.
    if let Some((title, body)) = pending_tool.take() {
        let _ = renderer.commit_panel(&title, &body, false);
    }
  ```

## Spec

### Task 1 — Add the three keys

In `src/cli/input/tty.rs`:

- Add `CtrlO`, `PageUp`, `PageDown` to the `Key` enum (near `CtrlK`/`Home`).
- Add `b'\x0f' => Key::CtrlO,` alongside the other control-byte arms.
- Add CSI arms for `ESC[5~` → `Key::PageUp` and `ESC[6~` → `Key::PageDown`,
  each consuming the trailing `~` exactly as the `b'3'` Delete arm does.
- Every existing `match key` over `Key` must still compile; add no-op arms
  where a match is exhaustive.

### Task 2 — Create the viewer's pure layout

Create `src/cli/viewer.rs`. Start with the **pure**, unit-testable half:

```rust
/// What a rendered row is, so the draw pass can style it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Header,
    User,
    Assistant,
    Tool,
    Output,
    System,
    Blank,
}

/// One laid-out screen row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewRow {
    pub text: String,
    pub kind: RowKind,
}
```

`pub fn layout_blocks(blocks: &[Block], width: usize) -> Vec<ViewRow>` lays the
transcript out at `width` columns. Pin the shape exactly — the tests assert on
it:

- `UserTurn { label, text }` → one `Header` row `▸ {label}`, then `text`
  hard-wrapped to `width` as `User` rows.
- `Assistant { text }` → `text` hard-wrapped as `Assistant` rows.
- `ToolPanel { tool, summary, label }` → one `Header` row
  `▸ {tool}{ — label}` (the ` — {label}` suffix only when `label` is `Some`),
  then `summary` hard-wrapped as `Tool` rows.
- `Output { full, .. }` → one `Header` row `output ({n} lines)` where `n` is
  `full.lines().count()`, then **every** line of `full`, hard-wrapped, as
  `Output` rows. Never elide here: showing what the inline panel hid is the
  whole point of the viewer.
- `System { text }` → `text` hard-wrapped as `System` rows, each prefixed `⚙ `
  on the first row only.
- Exactly one `Blank` row (empty text) **between** blocks — not after the last.

Wrap with the existing helper `crate::cli::render::wrap_line_hard(s, width)`
(`src/cli/render.rs:4`), which is what the inline panel body uses. An empty
`full`/`text` still emits its `Header` row and no body rows.

Then the scroll clamp — write it **exactly** in this shape, task 9's mutation
targets it verbatim:

```rust
/// Clamp a scroll offset so the last page never scrolls past the end.
pub fn clamp_scroll(scroll: usize, total: usize, height: usize) -> usize {
    let max = total.saturating_sub(height);
    scroll.min(max)
}
```

### Task 3 — The draw pass

In `src/cli/viewer.rs`, add a draw function that renders `rows` into a frame at
a given scroll offset:

```rust
pub fn render_transcript(
    f: &mut ratatui::Frame,
    rows: &[ViewRow],
    scroll: usize,
    evicted: usize,
)
```

- The bottom row is a status line reading
  `transcript — {shown_from}-{shown_to} of {total} lines · ↑↓ PgUp/PgDn Home/End · esc to close`,
  and when `evicted > 0` it is prefixed `{evicted} older blocks evicted · `.
- The remaining rows render `rows[scroll..]`, one `ViewRow` per screen row,
  styled by `RowKind` from `crate::cli::palette::Palette::from_env()`. Styling
  choices are yours — **pin nothing about colors**; the tests assert on text
  only.
- It must not panic when `rows` is empty or `scroll` exceeds `rows.len()` —
  render whatever is in range.

### Task 4 — The viewer loop

In `src/cli/viewer.rs`:

```rust
pub async fn run_transcript_viewer(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    transcript: &crate::cli::transcript::Transcript,
) -> anyhow::Result<()>
```

Sequence, in order:

1. `execute!(std::io::stdout(), EnterAlternateScreen)` (crossterm).
2. Build a **fullscreen** terminal over stdout:
   `Terminal::new(CrosstermBackend::new(std::io::stdout()))`.
3. Lay out with `layout_blocks(transcript.blocks(), width)` where `width` is
   the terminal's current width, and start scrolled to the **bottom**
   (`clamp_scroll(usize::MAX, rows.len(), body_height)`) — the user opens the
   viewer to see what just happened.
4. Loop on a `tokio::select!` over `sigwinch.recv()` and `read_key(stdin)`,
   mirroring the existing loop at `chat.rs:633`:
   - `sigwinch` → re-query size, **re-run `layout_blocks` at the new width**,
     re-clamp scroll, redraw. Reflow on resize is a capability the inline
     surface does not have; this is where it comes from.
   - `Key::Up` / `Key::Down` → ±1 row. `Key::PageUp` / `Key::PageDown` →
     ± (body height − 1). `Key::Home` → 0. `Key::End` → bottom.
   - `Key::Char('\x1b')` (bare Escape — see Current state), `Key::Char('q')`,
     or `Key::CtrlO` → break.
   - Every scroll passes through `clamp_scroll`; the offset is never allowed
     out of range.
5. On break: drop the fullscreen terminal, then
   `execute!(std::io::stdout(), LeaveAlternateScreen)`, then
   `renderer.reanchor()`.

**Do not call `ratatui::try_restore()`, `restore()`, or
`disable_raw_mode()` anywhere in this file.** The chat session continues in raw
mode after the viewer closes; calling any of them leaves the terminal cooked
and the session unusable. Task 10's negative grep enforces this.

### Task 5 — Register the module

In `src/cli/mod.rs`, add `pub mod viewer;` after `pub mod transcript;`. No glob
re-export.

### Task 6 — Wire ctrl+o into the chat input loop

In `src/cli/commands/chat.rs`:

- Add `transcript: &'a crate::cli::transcript::Transcript,` to
  `RatatuiInputCtx` (line 597) and destructure it in
  `read_input_line_inner_ratatui`.
- Pass `transcript: &transcript` at the call site (line 397).
- Add a `Key::CtrlO` arm to the key match that calls
  `crate::cli::viewer::run_transcript_viewer(stdin, sigwinch, renderer,
  transcript).await`, then redraws the live region exactly as the sigwinch arm
  does (`renderer.draw(state.current_line(), &sb)` with the same
  `StatusBarState`). The input line's contents must survive the round trip
  untouched — the viewer never mutates `state`.

### Task 7 — Close carried gap 1: enforce the byte cap while streaming

In `src/cli/transcript.rs`, make `append_assistant` enforce the byte budget on
the coalescing path too. Constraint, not implementation: after any
`append_assistant` call, `bytes` must satisfy the same invariant `push`
guarantees, and the block being appended to must **not** be evicted out from
under the caller when it is the only block. Add the test named in § Test plan.

### Task 8 — Close carried gap 2: record the two unrecorded panels

In `src/cli/commands/stream.rs`, push a `Block::ToolPanel` in the two places
that commit a panel without recording one:

- the `ToolFinished` arm's `None` branch (line 678), which commits a bare
  `result` panel; and
- the end-of-turn flush of a started-but-never-finished tool (lines 729-732).

Use the same title/body the `commit_panel*` call receives, so the viewer shows
what scrollback shows.

### Task 9 — Mutation M1: apply

Use the `patch` tool on `src/cli/viewer.rs` to break the scroll clamp.

- `old_str`: `    let max = total.saturating_sub(height);`
- `new_str`: `    let max = total;`

Then run, appending to the evidence artifact:

```sh
A=/tmp/e2e-02.txt
echo "== M1 APPLIED ==" >> "$A"
grep -c 'let max = total;' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

The run **must fail** — `clamp_scroll_pins_to_last_page` is what proves the
clamp is real. A green run means the test is vacuous; stop and file a blocker.

### Task 10 — Mutation M1: restore

`patch` the same line back (`old_str`/`new_str` swapped), then:

```sh
A=/tmp/e2e-02.txt
echo "== M1 RESTORED ==" >> "$A"
grep -c 'let max = total;' src/cli/viewer.rs >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "exit=${PIPESTATUS[0]}" >> "$A"
```

`grep -c` must print `1` after task 9 and `0` after task 10. Do **not** use
`git checkout` to restore — the file holds this round's uncommitted work.

### Task 11 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-02.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

### Task 12 — PASTE MATCH self-check

After pasting, run:

```sh
D=docs/dev/milestones/M17-transcript-view/phase-02-viewer-shell.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02.txt
diff /tmp/pasted-02.txt /tmp/e2e-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the literal verdict line into that same Update Log entry, below the
fence.

## Acceptance criteria

- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` passes.
- [ ] `grep -c "EnterAlternateScreen" src/cli/viewer.rs` prints 1 and
      `grep -c "LeaveAlternateScreen" src/cli/viewer.rs` prints 1.
- [ ] `grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs`
      prints nothing and exits 1 — the viewer never tears down raw mode.
- [ ] `grep -c "reanchor()" src/cli/viewer.rs` prints at least 1.
- [ ] `grep -c "Key::CtrlO" src/cli/input/tty.rs` prints at least 1, and
      `grep -c "0x0f\|\\\\x0f" src/cli/input/tty.rs` prints at least 1.
- [ ] Tests `layout_blocks_renders_full_output`,
      `layout_blocks_separates_blocks_with_one_blank`,
      `layout_blocks_wraps_to_width`,
      `layout_blocks_empty_transcript_is_empty`,
      `clamp_scroll_pins_to_last_page`,
      `clamp_scroll_zero_when_content_fits`,
      `render_transcript_draws_rows_into_backend`,
      `render_transcript_survives_scroll_past_end`, and
      `append_assistant_enforces_byte_cap` all pass.
- [ ] `/tmp/e2e-02.txt` shows `== M1 APPLIED ==` with a **failing** run and
      `grep -c` = 1, then `== M1 RESTORED ==` with a passing run and
      `grep -c` = 0.
- [ ] The Update Log's newest entry is headed
      `### Update — <date> (end-to-end verification)`, contains the pasted
      artifact, and ends with the literal line `PASTE MATCH`.

## Test plan

In `src/cli/viewer.rs` (`#[cfg(test)] mod tests`):

- `layout_blocks_renders_full_output` — a `Block::Output` whose `full` has 300
  lines and `shown: 9` produces a `Header` row reading `output (300 lines)`
  followed by 300 `Output` rows. **The elision must not survive into the
  viewer**; assert the row count, not just the header.
- `layout_blocks_separates_blocks_with_one_blank` — two blocks produce exactly
  one `RowKind::Blank` row, and it is not the last row.
- `layout_blocks_wraps_to_width` — a 100-char `Assistant` text at `width = 20`
  produces rows each of at most 20 chars, and rejoining them reproduces the
  original text.
- `layout_blocks_empty_transcript_is_empty` — `layout_blocks(&[], 80)` returns
  an empty `Vec`, no blank row.
- `clamp_scroll_pins_to_last_page` — `clamp_scroll(9999, 100, 10) == 90`, and
  `clamp_scroll(50, 100, 10) == 50` (an in-range offset is untouched).
- `clamp_scroll_zero_when_content_fits` — `clamp_scroll(5, 3, 10) == 0`; the
  negative case that `saturating_sub` exists for.
- `render_transcript_draws_rows_into_backend` — with
  `Terminal::new(TestBackend::new(40, 8))`, draw rows whose texts are
  distinguishable and assert a mid-list row's text appears in the buffer and
  the status line's `of {total} lines` appears on the bottom row.
- `render_transcript_survives_scroll_past_end` — same backend, `scroll`
  greater than `rows.len()`; the call must not panic and must still draw the
  status line.

In `src/cli/transcript.rs`:

- `append_assistant_enforces_byte_cap` — with `with_caps(usize::MAX, 64)`,
  push a `System` block then `append_assistant` a 200-byte string; assert the
  store's byte accounting did not exceed the cap unbounded (the older block is
  evicted) **and** that the assistant block itself survives.

## End-to-end verification

The viewer's real behaviour — alternate screen enter/exit, and the inline
surface surviving the round trip — is a **live** check in a real tmux pane and
is architect-run at milestone close per the M14/M15/M16 convention (the
milestone README's exit criteria carry it). What the executor verifies here is
everything reachable headlessly: the pure layout, the clamp, a real
`TestBackend` draw, and the structural greps that pin the enter/exit contract.

Tasks 9 and 10 append the mutation pair to the **same** artifact before this
block runs; do not truncate `/tmp/e2e-02.txt` here.

```sh
A=/tmp/e2e-02.txt
echo "== GATES ==" >> "$A"
cargo fmt --all -- --check 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "fmt exit=${PIPESTATUS[0]}" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -5 >> "$A"
echo "clippy exit=${PIPESTATUS[0]}" >> "$A"
cargo test 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -25 >> "$A"
echo "test exit=${PIPESTATUS[0]}" >> "$A"
echo "== VIEWER UNITS ==" >> "$A"
cargo test --lib cli::viewer 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -20 >> "$A"
echo "units exit=${PIPESTATUS[0]}" >> "$A"
echo "== TRANSCRIPT UNITS ==" >> "$A"
cargo test --lib cli::transcript 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -15 >> "$A"
echo "transcript exit=${PIPESTATUS[0]}" >> "$A"
echo "== ALT-SCREEN CONTRACT ==" >> "$A"
grep -c "EnterAlternateScreen" src/cli/viewer.rs >> "$A"
grep -c "LeaveAlternateScreen" src/cli/viewer.rs >> "$A"
grep -c "reanchor()" src/cli/viewer.rs >> "$A"
echo "== NO RAW-MODE TEARDOWN IN VIEWER ==" >> "$A"
grep -nE "try_restore|disable_raw_mode|\.restore\(\)" src/cli/viewer.rs >> "$A"
echo "teardown grep exit=$?  (1 = none found, which is the pass)" >> "$A"
echo "== KEY WIRING ==" >> "$A"
grep -n "CtrlO\|PageUp\|PageDown" src/cli/input/tty.rs >> "$A"
echo "keys exit=$?" >> "$A"
```

## Authorizations

- [ ] May add the file `src/cli/viewer.rs` and register it in `src/cli/mod.rs`.
- [ ] May extend the `Key` enum and `read_key` in `src/cli/input/tty.rs`.
- [ ] May edit `src/cli/commands/chat.rs` (the input-loop context, its call
      site, and the new `Key::CtrlO` arm only).
- [ ] May edit `src/cli/transcript.rs` (task 7) and
      `src/cli/commands/stream.rs` (task 8).

No new dependencies — `ratatui` 0.30 and `crossterm` 0.29 are already present.
`docs/architecture.md` is **not** authorized.

## Out of scope

- **`src/cli/render_ratatui.rs`.** The inline renderer is not modified. The
  viewer calls its existing `reanchor()`; it changes nothing inside it.
- **Expand/collapse and the inline `· ctrl+o` footer hint** — phase-03. Every
  block renders in full here, and the inline `… N more lines` text is
  unchanged.
- **Search** (phase-04), **copy / `tmux load-buffer`** (phase-05),
  **rehydration from the session JSONL** (phase-06), **mouse and wheel
  scrolling** (phase-07). No SGR mouse enabling in this phase.
- **Opening the viewer mid-turn.** `ctrl+o` is handled only in the idle input
  loop; the streaming loop in `stream.rs` is not given a viewer path.
- **Reading `var/log/panes/*.log`** — that archive is unmasked, and the viewer
  renders only what the wire delivered.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
