# Phase 05a: tail-model — the `/tail` core: the shell's size on the wire, a follow/scroll model, and a grid painter

**Milestone:** M23 — Client decoupling & shell UX
**Status:** review
**Depends on:** 03d (`ShellTail`), 04c (done)
**Estimated diff:** ~200 lines of production code across 4 files (1 new); 7 new tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Row 05 is `/tail <sN>`: a live view of one shell inside the alt-screen viewer,
with `f` toggling follow, `/` searching and `Esc` returning. This phase builds
its **testable core**; 05b wires the viewer loop, the command and search.

After this phase:

1. **`Response::ShellScreen` carries the shell's size.** The screen bytes
   assume the shell's own `rows × cols`; a client cannot draw them without
   it. The relay asks the host's `Status` first and sends
   `ShellScreen { bytes, rows, cols }`.
2. **`crate::cli::tail::TailModel`** rebuilds the shell's terminal from a
   `ShellTail` stream in a `vt100::Parser` of that size, with 5 000 lines of
   scrollback. `follow` pins the view to the newest output; scrolling up
   pauses it and the view then stays put while output arrives; scrolling back
   to the bottom (or `f`) resumes it. `ShellExited` marks the model exited.
3. **`crate::cli::tail::render_tail`** paints the grid into a ratatui frame
   cell by cell (text, 16 ANSI colours by name so the user's theme applies,
   256/RGB passed through, bold/italic/underline/inverse) with a status line.
   A grid wider or taller than the terminal is clipped to the top-left.

Everything here is pure or `TestBackend`-testable: no alt screen, no key
loop, no daemon in the tests except the one relay test.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 — `/tail` through the vt100 grid.
- `docs/dev/milestones/M23-client-decoupling/README.md` — rows 05a/05b.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-02** at commit `5d6181d7`. Baseline:
`cargo test --lib` → **2133 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2140**/0/14 on three runs, every other target
  unmoved;
- fmt and clippy `-D warnings` were clean;
- Mutations A–F each went red;
- one run of 19 failed `daemon::utils::event_log::tests::log_event_always_stamps_ts_event_and_pid`;
  an A/B of 8 full runs each (dry run vs clean commit) was 0/8 vs 0/8. A
  one-off, unrelated to this phase.

### vt100 0.16.2 facts (measured in a scratch crate)

- `Parser::new(rows, cols, scrollback)`; `screen().size()` is `(rows, cols)`.
- `screen_mut().set_scrollback(n)` scrolls the **view** `n` lines back into
  history and is clamped to what exists; `screen().scrollback()` reads it
  back; `0` is the live screen. **While scrolled back, new output does not
  move the view**: with 30 lines in a 5-row grid, `set_scrollback(3)` showed
  `line 23` at the top, a further `fresh\r\n` left it at `line 23` with the
  offset now 4, and `set_scrollback(0)` showed the bottom including `fresh`.
  That is exactly the follow/pause behaviour, so the model only has to
  decide **when** to call `set_scrollback(0)`.
- `screen().cell(row, col) -> Option<&Cell>`; `Cell` has `contents() -> &str`
  (empty for a blank), `fgcolor()`/`bgcolor() -> vt100::Color`
  (`Default | Idx(u8) | Rgb(u8,u8,u8)`), `bold()`, `italic()`, `underline()`,
  `inverse()`.

### ratatui 0.30 facts

`ratatui` is a facade over `ratatui-core 0.1.2`. What the painter uses:
- `Frame::buffer_mut() -> &mut Buffer`; `Buffer::cell_mut((x, y)) -> Option<&mut Cell>`
  (`None` outside the buffer, so clipping needs no bounds arithmetic beyond
  the loop limits);
- `Cell::set_symbol(&str)`, `set_char(char)`, `set_style(Style)`;
- `Color::{Reset, Black, Red, Green, Yellow, Blue, Magenta, Cyan, Gray,
  DarkGray, LightRed, LightGreen, LightYellow, LightBlue, LightMagenta,
  LightCyan, White, Indexed(u8), Rgb(u8,u8,u8)}`;
- `Modifier::{BOLD, ITALIC, UNDERLINED, REVERSED, DIM}`.

### The analogue: the transcript viewer (`src/cli/viewer.rs`)

`render_transcript` (`:474`) paints into `f.area()` with a one-row status
line at the bottom, dimmed. Its test
`render_transcript_draws_rows_into_backend` (`:979`) draws into
`Terminal::new(TestBackend::new(40, 8))`, then reads
`terminal.backend().buffer().content` and joins `c.symbol()` per row to
assert on text. The new tests use the same shape, and also read `.fg` and
`.modifier` on a cell.

### The relay today (`src/daemon/server/shells.rs`)

`handle_shell_tail` computes `let sock = crate::shell::socket_path(id);`
(`:155`), connects, subscribes, then sends
`Response::ShellScreen { bytes: screen }` (`:177`). The host's `Status`
answer (`crate::shell::status(&sock, RELAY_TIMEOUT)`) carries `rows` and
`cols`; 03a's resize test reads them the same way.

### Other `ShellScreen` sites that must change with the variant

- `src/ipc.rs:518-520` (the variant) and `:830` (`kind()`, uses `{ .. }`,
  unchanged);
- `src/ipc_tests.rs:1102-1110` (the round trip);
- `src/daemon/server/shells.rs:668` and `:879` (two tests destructure
  `{ bytes }`: add `, ..`);
- `src/cli/commands/ask.rs:218` and `stream.rs:757` use `{ .. }`,
  unchanged.

## Spec

### Task 1 — the size on the wire (`src/ipc.rs`, `src/ipc_tests.rs`)

Replace the `ShellScreen` variant and its doc comment (`:518-520`) with:

```rust
    /// The first frame of a `ShellTail`: escape codes that redraw the shell's
    /// current screen, and the shell's terminal size, which those codes assume.
    /// `bytes` is empty when the shell's host is too old to provide a screen.
    ShellScreen {
        bytes: Vec<u8>,
        rows: u16,
        cols: u16,
    },
```

In `shell_tail_messages_round_trip` (`ipc_tests.rs:1102-1110`), build the
screen with `rows: 30, cols: 100` and assert they survive:

```rust
    let screen = Response::ShellScreen {
        bytes: vec![0x1b, b'[', b'H', 0xff],
        rows: 30,
        cols: 100,
    };
    match roundtrip_resp(&screen) {
        Response::ShellScreen { bytes, rows, cols } => {
            assert_eq!(
                bytes,
                vec![0x1b, b'[', b'H', 0xff],
                "screen bytes survive the wire"
            );
            assert_eq!((rows, cols), (30, 100), "the size survives the wire");
        }
        other => panic!("wrong variant: {other:?}"),
    }
```

### Task 2 — the relay (`src/daemon/server/shells.rs`)

1. Right after `let sock = crate::shell::socket_path(id);` (`:155`), add:

```rust
    // The screen bytes assume the shell's own terminal size; the client needs
    // it to draw them. A host that cannot answer reads as the spawn default.
    let (rows, cols) = match crate::shell::status(&sock, RELAY_TIMEOUT).await {
        Ok(crate::shell::ShellResponse::Status { rows, cols, .. }) => (rows, cols),
        _ => (24, 80),
    };
```

2. Replace the `ShellScreen` send (`:177`, the `if send_response_split(tx,
   Response::ShellScreen { bytes: screen })` head) so the `if` reads:

```rust
    if send_response_split(
        tx,
        Response::ShellScreen {
            bytes: screen,
            rows,
            cols,
        },
    )
    .await
    .is_err()
            if let [Response::ShellScreen { bytes, .. }] = &parse_all(&tx)[..]
```

   (its `.await.is_err()` body and `return Ok(());` stay as they are).

3. In the two tests that destructure the variant (`:668`, `:879`), change
   `Response::ShellScreen { bytes }` to `Response::ShellScreen { bytes, .. }`.

4. Add this test right **above** `async fn shell_tail_follow_streams_live_chunks_until_the_client_disconnects`
   (`:691`, with its `#[tokio::test]` line):

```rust
#[tokio::test]
async fn shell_tail_reports_the_shells_size() {
    let (_guard, _dir) = shell_test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("t".into());
    let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
        Box::pin(spawn_in_process_host(id))
    })
    .await
    .unwrap();
    crate::shell::send_resize(
        &crate::shell::socket_path(id.as_str()),
        30,
        100,
        RELAY_TIMEOUT,
    )
    .await
    .unwrap();
    let mut rx = tokio::io::BufReader::new(tokio::io::empty());
    let mut tx: Vec<u8> = Vec::new();
    handle_shell_tail(&mut rx, &mut tx, &reg, id.as_str(), false)
        .await
        .unwrap();
    match parse_all(&tx).as_slice() {
        [Response::ShellScreen { rows, cols, .. }] => {
            assert_eq!(
                (*rows, *cols),
                (30, 100),
                "the shell's own size, not a default"
            )
        }
        other => panic!("expected one ShellScreen, got {other:?}"),
    }
    close_tail_shell(&reg, id.as_str(), &owner).await;
}
```

### Task 3 — the model and the painter (`src/cli/tail.rs`, new; `src/cli/mod.rs`)

Add `pub mod tail;` after `pub mod status;` in `src/cli/mod.rs` (`:12`).
Create `src/cli/tail.rs` with exactly this content (the dry-run file,
formatted and clippy-clean; its tests are Task 4):

```rust
//! `/tail`'s model: one shell's terminal, rebuilt in the client from a
//! `ShellTail` stream (`ShellScreen`, then `ShellChunk`s, then `ShellExited`).
//! Pure state, no terminal I/O, so every rule here is tested without a
//! daemon or a screen.

use crate::ipc::Response;

/// Lines of history the client keeps for scrolling back. The shell's own
/// history before the tail started is not sent; this holds what arrives.
pub const TAIL_SCROLLBACK: usize = 5_000;

/// One tailed shell. `follow` keeps the view pinned to the newest output;
/// scrolling up pauses it, and the view then stays put while output arrives.
pub struct TailModel {
    pub id: String,
    parser: vt100::Parser,
    pub follow: bool,
    pub exited: bool,
}

impl TailModel {
    /// The model for `id`, from the stream's first frame: a grid of the
    /// shell's own `rows` × `cols`, drawn from `screen`.
    pub fn new(id: &str, rows: u16, cols: u16, screen: &[u8]) -> Self {
        let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), TAIL_SCROLLBACK);
        parser.process(screen);
        Self {
            id: id.to_string(),
            parser,
            follow: true,
            exited: false,
        }
    }

    /// Apply one later frame of the stream. Returns `true` when the view may
    /// have changed and should be redrawn.
    pub fn apply(&mut self, frame: &Response) -> bool {
        match frame {
            Response::ShellChunk { bytes } => {
                self.parser.process(bytes);
                if self.follow {
                    self.parser.screen_mut().set_scrollback(0);
                }
                true
            }
            Response::ShellExited => {
                self.exited = true;
                true
            }
            _ => false,
        }
    }

    /// Follow on: jump to the newest output. Follow off: hold the view where
    /// it is.
    pub fn toggle_follow(&mut self) {
        self.follow = !self.follow;
        if self.follow {
            self.parser.screen_mut().set_scrollback(0);
        }
    }

    /// Scroll back `n` lines into history. Scrolling back pauses follow.
    pub fn scroll_up(&mut self, n: usize) {
        let to = self.offset().saturating_add(n);
        self.parser.screen_mut().set_scrollback(to);
        if self.offset() > 0 {
            self.follow = false;
        }
    }

    /// Scroll `n` lines toward the newest output. Reaching it resumes follow.
    pub fn scroll_down(&mut self, n: usize) {
        let to = self.offset().saturating_sub(n);
        self.parser.screen_mut().set_scrollback(to);
        if to == 0 {
            self.follow = true;
        }
    }

    /// How many lines back from the newest output the view is.
    pub fn offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    /// The grid as it is currently in view.
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
}

/// Paint `model`'s grid into the frame, cell by cell, with a one-line status
/// bar at the bottom. The grid keeps the shell's own size: when the terminal
/// is smaller, the top-left corner is shown; when it is larger, the rest is
/// left blank. Pure over `Frame`, so `TestBackend` tests cover it.
pub fn render_tail(f: &mut ratatui::Frame, model: &TailModel) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};

    let area = f.area();
    let body_height = area.height.saturating_sub(1);
    let screen = model.screen();
    let (rows, cols) = screen.size();
    let buf = f.buffer_mut();
    for row in 0..rows.min(body_height) {
        for col in 0..cols.min(area.width) {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let Some(out) = buf.cell_mut((area.x + col, area.y + row)) else {
                continue;
            };
            let text = cell.contents();
            if text.is_empty() {
                out.set_char(' ');
            } else {
                out.set_symbol(text);
            }
            let mut style = Style::default()
                .fg(color(cell.fgcolor()))
                .bg(color(cell.bgcolor()));
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.inverse() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            out.set_style(style);
        }
    }

    let mode = if model.exited {
        "exited".to_string()
    } else if model.follow {
        "following".to_string()
    } else {
        format!("paused · {} lines back", model.offset())
    };
    let status = format!(
        "tail {} · {rows}×{cols} · {mode} · f follow · ↑↓ PgUp/PgDn · esc to close",
        model.id
    );
    f.render_widget(
        Line::from(Span::styled(
            status,
            Style::default().add_modifier(Modifier::DIM),
        )),
        ratatui::layout::Rect::new(area.x, area.y + body_height, area.width, 1),
    );
}

/// A vt100 colour as ratatui draws it. The 16 ANSI colours map by name so the
/// user's terminal theme applies; 256-colour and RGB pass through.
fn color(c: vt100::Color) -> ratatui::style::Color {
    use ratatui::style::Color as R;
    match c {
        vt100::Color::Default => R::Reset,
        vt100::Color::Idx(0) => R::Black,
        vt100::Color::Idx(1) => R::Red,
        vt100::Color::Idx(2) => R::Green,
        vt100::Color::Idx(3) => R::Yellow,
        vt100::Color::Idx(4) => R::Blue,
        vt100::Color::Idx(5) => R::Magenta,
        vt100::Color::Idx(6) => R::Cyan,
        vt100::Color::Idx(7) => R::Gray,
        vt100::Color::Idx(8) => R::DarkGray,
        vt100::Color::Idx(9) => R::LightRed,
        vt100::Color::Idx(10) => R::LightGreen,
        vt100::Color::Idx(11) => R::LightYellow,
        vt100::Color::Idx(12) => R::LightBlue,
        vt100::Color::Idx(13) => R::LightMagenta,
        vt100::Color::Idx(14) => R::LightCyan,
        vt100::Color::Idx(15) => R::White,
        vt100::Color::Idx(n) => R::Indexed(n),
        vt100::Color::Rgb(r, g, b) => R::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_tail_paints_the_grid_with_colours_and_a_status_line() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::{Color, Modifier};

        let mut m = TailModel::new("s3", 5, 20, b"");
        m.apply(&chunk(b"plain \x1b[1;31mred\x1b[0m\r\nsecond".to_vec()));
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|f| render_tail(f, &m)).unwrap();
        let buf = terminal.backend().buffer();
        let row0: String = buf
            .content
            .iter()
            .take(40)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(row0.starts_with("plain red"), "{row0:?}");
        let red = &buf.content[6];
        assert_eq!(red.symbol(), "r");
        assert_eq!(red.fg, Color::Red, "ANSI red maps by name");
        assert!(red.modifier.contains(Modifier::BOLD), "bold carries");
        assert_eq!(buf.content[0].fg, Color::Reset, "default colour is Reset");
        let row1: String = buf
            .content
            .iter()
            .skip(40)
            .take(40)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(row1.starts_with("second"), "{row1:?}");
        let bottom: String = buf
            .content
            .iter()
            .skip(7 * 40)
            .take(40)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            bottom.contains("tail s3") && bottom.contains("5×20") && bottom.contains("following"),
            "{bottom:?}"
        );
    }

    #[test]
    fn render_tail_clips_a_grid_wider_than_the_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut m = TailModel::new("s3", 3, 60, b"");
        m.apply(&chunk(format!("{}\r\n", "x".repeat(60)).into_bytes()));
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        terminal.draw(|f| render_tail(f, &m)).unwrap();
        let buf = terminal.backend().buffer();
        let row0: String = buf
            .content
            .iter()
            .take(20)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert_eq!(
            row0,
            "x".repeat(20),
            "the top-left corner shows, nothing panics"
        );
        let bottom: String = buf
            .content
            .iter()
            .skip(3 * 20)
            .take(20)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(bottom.starts_with("tail s3"), "{bottom:?}");
    }

    fn lines(n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| format!("line {i}\r\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn chunk(bytes: Vec<u8>) -> Response {
        Response::ShellChunk { bytes }
    }

    fn top_line(m: &TailModel) -> String {
        m.screen()
            .contents()
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn tail_model_draws_the_screen_at_the_shells_size() {
        let m = TailModel::new("s3", 30, 100, b"\x1b[1;34mhello\x1b[0m");
        assert_eq!(m.screen().size(), (30, 100), "the grid is the shell's size");
        assert!(
            m.screen().contents().starts_with("hello"),
            "{:?}",
            m.screen().contents()
        );
        assert!(m.follow, "a new tail follows");
        assert!(!m.exited);
    }

    #[test]
    fn tail_model_follows_new_output_until_scrolled_back() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        assert!(m.apply(&chunk(lines(30))));
        assert_eq!(top_line(&m), "line 26", "following shows the newest lines");
        m.scroll_up(3);
        assert!(!m.follow, "scrolling back pauses follow");
        assert_eq!(top_line(&m), "line 23");
        m.apply(&chunk(b"fresh\r\n".to_vec()));
        assert_eq!(
            top_line(&m),
            "line 23",
            "a paused view stays put while output arrives"
        );
        m.scroll_down(usize::MAX);
        assert!(m.follow, "scrolling back to the bottom resumes follow");
        assert!(
            m.screen().contents().contains("fresh"),
            "{:?}",
            m.screen().contents()
        );
    }

    #[test]
    fn tail_model_toggle_follow_jumps_to_the_newest_output() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        m.apply(&chunk(lines(30)));
        m.scroll_up(10);
        m.apply(&chunk(b"newest\r\n".to_vec()));
        assert!(!m.screen().contents().contains("newest"));
        m.toggle_follow();
        assert!(m.follow);
        assert_eq!(m.offset(), 0);
        assert!(
            m.screen().contents().contains("newest"),
            "{:?}",
            m.screen().contents()
        );
        m.toggle_follow();
        assert!(!m.follow, "toggling again pauses");
    }

    #[test]
    fn tail_model_marks_the_shell_exited() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        assert!(m.apply(&Response::ShellExited));
        assert!(m.exited);
        assert!(!m.apply(&Response::Ok), "other frames change nothing");
    }
}
```

### Task 4 — tests

Seven tests, all included above, names pinned:
- `shell_tail_messages_round_trip` (updated, Task 1);
- `shell_tail_reports_the_shells_size` (Task 2.4);
- in `tail.rs`: `tail_model_draws_the_screen_at_the_shells_size`,
  `tail_model_follows_new_output_until_scrolled_back`,
  `tail_model_toggle_follow_jumps_to_the_newest_output`,
  `tail_model_marks_the_shell_exited`,
  `render_tail_paints_the_grid_with_colours_and_a_status_line`,
  `render_tail_clips_a_grid_wider_than_the_terminal`.

**Test count:** seven new, so **2140 passed / 14 ignored**.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-05a.txt`**: the
file's lines, copied by a tool. **Never paste the script itself**, and never
retype the lines (the real lines end `lib1=ok ` **with a trailing space**).
Then run the paste check once, put the line it prints into the entry **on its
own line**, and record the six mutations after it.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today, and each was run at drafting.

- [ ] `cargo test --lib` → **2140 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2133/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The seven tests pass:
      ``cargo test --lib -- tail_model render_tail shell_tail_reports_the_shells_size shell_tail_messages_round_trip``
      → **7 passed** (current: 1 passed, the old round trip).
- [ ] The module exists and is wired:
      ``grep -c '^pub mod tail;' src/cli/mod.rs`` → **1** (current: 0), and
      ``test -f src/cli/tail.rs && echo yes`` → `yes` (current: no file).
- [ ] Mutations A–F were applied, run, and their observed results recorded
      **in this doc**.
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0). Its first fenced block is the capture file's contents,
      and the paste-check line is on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`). Run once, restore, and **record the
observed result**. Each was measured red at drafting.

- **Mutation A** (`shells.rs`): change
  `Ok(crate::shell::ShellResponse::Status { rows, cols, .. }) => (rows, cols),`
  to `Ok(crate::shell::ShellResponse::Status { .. }) => (24, 80),`. Run
  `shell_tail_reports_the_shells_size`. Expected **red**.
- **Mutation B** (`tail.rs`, `scroll_up`): delete the
  `if self.offset() > 0 { self.follow = false; }` block. Run
  `tail_model_follows_new_output_until_scrolled_back`. Expected **red**.
- **Mutation C** (`tail.rs`, `toggle_follow`): delete its
  `if self.follow { self.parser.screen_mut().set_scrollback(0); }` block.
  Run `tail_model_toggle_follow_jumps_to_the_newest_output`. Expected **red**.
- **Mutation D** (`tail.rs`, `scroll_down`): delete the
  `if to == 0 { self.follow = true; }` block. Run
  `tail_model_follows_new_output_until_scrolled_back`. Expected **red**.
- **Mutation E** (`tail.rs`, `color`): change `vt100::Color::Idx(1) => R::Red,`
  to `vt100::Color::Idx(1) => R::Reset,`. Run
  `render_tail_paints_the_grid_with_colours_and_a_status_line`. Expected
  **red**.
- **Mutation F** (`tail.rs`, `render_tail`): replace the
  `if text.is_empty() { out.set_char(' '); } else { out.set_symbol(text); }`
  block with `let _ = text;`. Run `render_tail`. Expected **red, 2 failed**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Seven tests:
- the size survives the wire;
- the relay reports the shell's real size (a resized in-process shell);
- the model draws at the shell's size and starts following;
- follow pauses on scroll-up, holds while output arrives, resumes at the bottom;
- `f` jumps to the newest output and toggles;
- `ShellExited` is recorded;
- the painter's text, colours, modifiers and status line, and its clipping.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- tail_model render_tail shell_tail_reports_the_shells_size shell_tail_messages_round_trip 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s\n' "$(grep -c '^pub mod tail;' src/cli/mod.rs)" "$(test -f src/cli/tail.rs && echo 1 || echo 0)"
} > /tmp/e2e-m23-05a.txt
cat /tmp/e2e-m23-05a.txt
```

Expected:
- `lib1..3=ok` with **nothing after `ok`**;
- `lib_counts=2140p-0f-14i`;
- `new_tests=7p-0f`;
- `isolation=11 passed; 0 failed`;
- `wired=1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-05a.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-05a-tail-model.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-05a.txt
diff /tmp/pasted-m23-05a.txt /tmp/e2e-m23-05a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-05a.txt /tmp/e2e-m23-05a.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line.

## Authorizations

You may edit these files, and nothing else:

- `src/ipc.rs` and `src/ipc_tests.rs`: Task 1.
- `src/daemon/server/shells.rs`: Task 2.
- `src/cli/mod.rs` (one line) and `src/cli/tail.rs` (new): Task 3.
- Any file the compiler names for a `ShellScreen { bytes }` destructure
  that stops compiling: add `, ..` only.
- This phase doc's Update Log.

## Out of scope

- **The viewer loop, `/tail` / `/watch`, search, the help line**: row 05b.
- **Resizing the shell to the viewer**: `/tail` is read-only; it never sends
  `ShellResize`.
- **Scrollback before the tail started**: the host sends its live screen,
  not its history. `/log` (row 06) reads recordings.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-02 16:05 (started)

**By:** executor (rexyMCP headless run)

Status flipped to `in-progress`; README row 05a matches. Beginning Task 1 (the
`ShellScreen { bytes, rows, cols }` variant) per the spec.

### Update — 2026-10-02 16:40 (end-to-end verification)

```
lib1=ok 
lib2=ok 
lib3=ok 
lib_counts=2140p-0f-14i
new_tests=8p-0f
isolation=11 passed; 0 failed
wired=1/1
```

PASTE MATCH

Mutations A–F, each applied with `patch`, run once, restored
(`git diff --stat` back to its pre-mutation state):

| mut | change | test | result |
|---|---|---|---|
| A | relay `Status` arm → `(24, 80)` | `shell_tail_reports_the_shells_size` | red |
| B | `scroll_up`: drop the pause | `tail_model_follows_new_output_until_scrolled_back` | red |
| C | `toggle_follow`: drop the jump to 0 | `tail_model_toggle_follow_jumps_to_the_newest_output` | red |
| D | `scroll_down`: drop the resume | `tail_model_follows_new_output_until_scrolled_back` | red |
| E | `Idx(1) => R::Reset` | `render_tail_paints_the_grid_with_colours_and_a_status_line` | red |
| F | `render_tail`: drop the symbol write | `render_tail` | red, 2 failed |

### Update — 2026-10-02 16:17 (complete, server-authored)

**Summary:** **Summary**

Phase 05a is implemented and committed on `master` (commits `662e4e80`, `0e08d9fb`, `5e2db21a`):

- **Task 1** — `Response::ShellScreen` now carries `bytes, rows, cols` (`src/ipc.rs`), with the wire round trip asserting `rows: 30, cols: 100` survive (`src/ipc_tests.rs`).
- **Task 2** — the relay asks the host's `Status` before tailing and falls back to `(24, 80)`, sends the size with the screen, the two destructuring tests gained `, ..`, and a new `shell_tail_reports_the_shells_size` test asserts a resized shell's real size through `handle_shell_tail`.
- **Task 3/4** — `src/cli/tail.rs` (new, wired after `pub mod status;` in `src/cli/mod.rs`) holds `TAIL_SCROLLBACK = 5_000`, `TailModel` (follow pins via `set_scrollback(0)`, scroll-up pauses, bottom/`f` resumes, `ShellExited` recorded) and `render_tail` (cell-by-cell painter, 16 ANSI colours by name, 256/RGB through, bold/italic/underline/inverse, dim status line, top-left clipping), with the six pinned tests.

**Deviations:** two, both mechanical. (1) The spec's Task-2.4 test body had `assert_eq!((*rows, *cols), (30, 100), …)` on one line; `cargo fmt` wrapped it — I committed the formatted form. (2) `new_tests=8p-0f` rather than the expected `7p-0f`: the spec's own filter `tail_model render_tail shell_tail_reports_the_shells_size shell_tail_messages_round_trip` matches 6 `tail.rs` tests + the relay test + the round trip = 8; the "7 new tests" count is right (the round trip is updated, not new), but the filter's arithmetic is 8. Everything else is verbatim.

**End-to-end verification:** the § block ran verbatim — `lib1..3=ok` with no failures, `lib_counts=2140p-0f-14i`, `new_tests=8p-0f`, `isolation=11 passed; 0 failed`, `wired=1/1`. Its capture (`/tmp/e2e-m23-05a.txt`) is pasted in the `### Update — 2026-10-02 16:40 (end-to-end verification)` entry inside a fenced block (raw lines would have been swallowed by the doc's own paste-extraction `awk`, which selects only fenced content — the fenced form makes the spec's paste check agree with the entry), the check printed `PASTE MATCH`, and the mutations table records A–F red. The no-fence draft of the entry was the one intermediate state that failed the spec's own check; it was fixed and re-checked before commit. No re-runs were needed: the named flake `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` never appeared, and no other test failed.

**Also verified:** `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` exits 0, `cargo test` → lib 2140/0/14 on three consecutive runs, bug_tracker 6, doc_truth 14, integration 32 (2 ignored), isolation 11, doc-tests 0; and `grep -c '^pub mod tail;' src/cli/mod.rs` → 1 with `src/cli/tail.rs` present.

**Notes for review:** the doc's `Status:` is still `in-progress` and the README row matches, per the lifecycle split — the completion flip is yours. The spec's mutation table says "red" per mutation as the phase requires; I did not paste per-mutation diffs. Next phase (05b) can take `TailModel`/`render_tail` as-is; its viewer loop only needs to feed frames into `apply` and translate keys to `toggle_follow`/`scroll_up`/`scroll_down`.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
_status_strips_trailing_prose ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
kill: sending signal to 3984095 failed: No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M23-client-decoupling/README.md` — +1 -1
- `docs/dev/milestones/M23-client-decoupling/phase-05a-tail-model.md` — +34 -1
- `src/cli/mod.rs` — +1 -0
- `src/cli/tail.rs` — +346 -0
- `src/daemon/server/shells.rs` — +54 -5
- `src/ipc.rs` — +7 -2
- `src/ipc_tests.rs` — +10 -5

**Commit:** 5e2db21a5b12ba3fa4b3e13009fa3fb5f9586a27

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
